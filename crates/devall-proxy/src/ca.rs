//! The per-user root CA used to decrypt HTTPS, and the leaf certificates it issues on the fly.
//!
//! The CA (ECDSA P-256, 10 years) lives in the app data directory — never inside a project:
//! `devall-ca.pem` (certificate), `devall-ca-key.pem` (PKCS#8 private key, mode 0600 on
//! macOS/Linux; on Windows the per-user profile ACL applies) and `devall-ca.json` (metadata).
//! Leaves are short-lived (180 days, below the 398-day browser limit), share one key per process
//! and are cached per host as ready-made rustls server configs.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData,
    SerialNumber,
};
use rustls::ServerConfig;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use time::OffsetDateTime;

use crate::error::{IoContext, ProxyError, Result};

pub const CA_CERT_FILE: &str = "devall-ca.pem";
pub const CA_KEY_FILE: &str = "devall-ca-key.pem";
pub const CA_META_FILE: &str = "devall-ca.json";
const ORGANIZATION: &str = "devall";
const ORG_UNIT: &str = "devall traffic inspector";
const CA_VALIDITY_DAYS: i64 = 3650;
const LEAF_VALIDITY_DAYS: i64 = 180;
const MAX_CACHED_HOSTS: usize = 2048;

/// The crypto provider used everywhere in this crate (explicit, so it never depends on which
/// provider another crate installed as the process default).
pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaMeta {
    version: u32,
    common_name: String,
    organization: String,
    org_unit: String,
    /// Unix seconds.
    created_at: i64,
    not_after: i64,
    algorithm: String,
}

/// What the UI shows about the CA.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaInfo {
    pub common_name: String,
    pub cert_path: String,
    pub dir: String,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    pub not_after: i64,
    /// SHA-256 of the certificate, `AA:BB:…`.
    pub sha256: String,
    /// SHA-1 of the certificate, uppercase hex without separators (what `certutil` and
    /// `security` use to identify it).
    pub sha1: String,
    /// Base64 SHA-256 of the SubjectPublicKeyInfo, for Chromium's
    /// `--ignore-certificate-errors-spki-list`.
    pub spki_sha256: String,
    pub pem: String,
}

pub struct CertAuthority {
    dir: PathBuf,
    meta: CaMeta,
    cert_pem: String,
    cert_der: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
    leaf_key: KeyPair,
    leaf_signer: Arc<dyn rustls::sign::SigningKey>,
    provider: Arc<CryptoProvider>,
    configs: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl std::fmt::Debug for CertAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertAuthority").field("dir", &self.dir).field("common_name", &self.meta.common_name).finish_non_exhaustive()
    }
}

fn cert_err(e: impl std::fmt::Display) -> ProxyError {
    ProxyError::Cert(e.to_string())
}

/// `devall local CA (<user>@<host>)`, at most 64 characters (the X.509 CN limit).
pub fn default_ca_name() -> String {
    let user = ["USER", "USERNAME", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty())).unwrap_or_else(|| "user".into());
    let host = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "localhost".into());
    let host = host.split('.').next().unwrap_or(&host).to_string();
    let mut name = format!("devall local CA ({}@{})", user.trim(), host.trim());
    if name.len() > 64 {
        name.truncate(name.floor_char_boundary(63));
        name.push(')');
    }
    name
}

fn random_serial() -> SerialNumber {
    let mut bytes = [0u8; 16];
    for b in &mut bytes {
        *b = fastrand::u8(..);
    }
    bytes[0] &= 0x7f;
    bytes[0] |= 0x01;
    SerialNumber::from_slice(&bytes)
}

fn ca_params(meta: &CaMeta) -> Result<CertificateParams> {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, meta.common_name.as_str());
    dn.push(DnType::OrganizationName, meta.organization.as_str());
    dn.push(DnType::OrganizationalUnitName, meta.org_unit.as_str());
    params.distinguished_name = dn;
    // Path length 0: this CA may only sign leaf certificates.
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    params.not_before = OffsetDateTime::from_unix_timestamp(meta.created_at - 86_400).map_err(cert_err)?;
    params.not_after = OffsetDateTime::from_unix_timestamp(meta.not_after).map_err(cert_err)?;
    Ok(params)
}

/// Refuses to keep the CA key anywhere inside a /dev/all project (a folder with `devall.yaml`).
fn ensure_outside_project(dir: &Path) -> Result<()> {
    let abs = if dir.is_absolute() { dir.to_path_buf() } else { std::env::current_dir().map(|c| c.join(dir)).unwrap_or_else(|_| dir.to_path_buf()) };
    for anc in abs.ancestors() {
        if anc.join(devall_core::project::MANIFEST).is_file() {
            return Err(ProxyError::Invalid(format!("refusing to store the CA private key inside the project {}", anc.display())));
        }
    }
    Ok(())
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    let tmp = path.with_extension(format!("{:08x}.tmp", fastrand::u32(..)));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).at(&tmp)?;
        f.write_all(data).at(&tmp)?;
        f.sync_all().at(&tmp)?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(ProxyError::IoAt { path: path.to_path_buf(), source: e });
    }
    Ok(())
}

fn hex_upper(bytes: &[u8], sep: &str) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(sep)
}

impl CertAuthority {
    /// Loads the CA from `dir`, creating a new one when none exists.
    pub fn load_or_create(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        if dir.join(CA_CERT_FILE).is_file() && dir.join(CA_KEY_FILE).is_file() && dir.join(CA_META_FILE).is_file() {
            Self::load(dir)
        } else {
            Self::create(dir, &default_ca_name())
        }
    }

    /// Generates a new CA in `dir`, replacing any existing one. Anything that trusted the old CA
    /// has to trust the new one again.
    pub fn create(dir: impl AsRef<Path>, common_name: &str) -> Result<Self> {
        let dir = dir.as_ref();
        ensure_outside_project(dir)?;
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(cert_err)?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let meta = CaMeta {
            version: 1,
            common_name: common_name.to_string(),
            organization: ORGANIZATION.into(),
            org_unit: ORG_UNIT.into(),
            created_at: now,
            not_after: now + CA_VALIDITY_DAYS * 86_400,
            algorithm: "ECDSA P-256 / SHA-256".into(),
        };
        let mut params = ca_params(&meta)?;
        params.serial_number = Some(random_serial());
        let cert = params.self_signed(&key).map_err(cert_err)?;
        fs::create_dir_all(dir).at(dir)?;
        write_private(&dir.join(CA_KEY_FILE), key.serialize_pem().as_bytes())?;
        devall_core::fsutil::atomic_write(&dir.join(CA_CERT_FILE), cert.pem().as_bytes())?;
        devall_core::fsutil::atomic_write(&dir.join(CA_META_FILE), serde_json::to_string_pretty(&meta)?.as_bytes())?;
        Self::assemble(dir, meta, cert.pem(), key)
    }

    fn load(dir: &Path) -> Result<Self> {
        ensure_outside_project(dir)?;
        let cert_pem = fs::read_to_string(dir.join(CA_CERT_FILE)).at(dir.join(CA_CERT_FILE))?;
        let key_pem = fs::read_to_string(dir.join(CA_KEY_FILE)).at(dir.join(CA_KEY_FILE))?;
        let meta: CaMeta = serde_json::from_slice(&fs::read(dir.join(CA_META_FILE)).at(dir.join(CA_META_FILE))?)?;
        let key = KeyPair::from_pem(&key_pem).map_err(cert_err)?;
        Self::assemble(dir, meta, cert_pem, key)
    }

    fn assemble(dir: &Path, meta: CaMeta, cert_pem: String, key: KeyPair) -> Result<Self> {
        let cert_der = CertificateDer::from_pem_slice(cert_pem.as_bytes()).map_err(|e| ProxyError::Cert(format!("{CA_CERT_FILE}: {e:?}")))?.into_owned();
        let issuer = Issuer::new(ca_params(&meta)?, key);
        let provider = provider();
        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(cert_err)?;
        let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
        let leaf_signer = provider.key_provider.load_private_key(der).map_err(cert_err)?;
        Ok(Self { dir: dir.to_path_buf(), meta, cert_pem, cert_der, issuer, leaf_key, leaf_signer, provider, configs: Mutex::new(HashMap::new()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn cert_path(&self) -> PathBuf {
        self.dir.join(CA_CERT_FILE)
    }

    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    pub fn common_name(&self) -> &str {
        &self.meta.common_name
    }

    /// Uppercase hex SHA-1 of the certificate (no separators).
    pub fn sha1_hex(&self) -> String {
        use sha1::Digest as _;
        hex_upper(&sha1::Sha1::digest(self.cert_der.as_ref()), "")
    }

    /// Base64 SHA-256 of the CA's SubjectPublicKeyInfo (Chromium's SPKI pin format).
    pub fn spki_sha256(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(self.issuer.key().subject_public_key_info()))
    }

    pub fn info(&self) -> CaInfo {
        CaInfo {
            common_name: self.meta.common_name.clone(),
            cert_path: self.cert_path().display().to_string(),
            dir: self.dir.display().to_string(),
            created_at: self.meta.created_at * 1000,
            not_after: self.meta.not_after * 1000,
            sha256: hex_upper(&sha2::Sha256::digest(self.cert_der.as_ref()), ":"),
            sha1: self.sha1_hex(),
            spki_sha256: self.spki_sha256(),
            pem: self.cert_pem.clone(),
        }
    }

    fn leaf_params(&self, hosts: &[&str]) -> Result<CertificateParams> {
        let names: Vec<String> = hosts.iter().map(|h| h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase()).collect();
        let mut params = CertificateParams::new(names.clone()).map_err(cert_err)?;
        let mut dn = DistinguishedName::new();
        let cn = names.first().cloned().unwrap_or_default();
        dn.push(DnType::CommonName, &cn[..cn.len().min(64)]);
        dn.push(DnType::OrganizationName, ORGANIZATION);
        params.distinguished_name = dn;
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let now = OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        let ca_end = OffsetDateTime::from_unix_timestamp(self.meta.not_after).map_err(cert_err)?;
        params.not_after = (now + time::Duration::days(LEAF_VALIDITY_DAYS)).min(ca_end);
        params.serial_number = Some(random_serial());
        params.use_authority_key_identifier_extension = true;
        Ok(params)
    }

    /// Issues a leaf for `hosts` (DNS names or IP addresses) signed with the shared leaf key.
    pub fn issue_leaf_der(&self, hosts: &[&str]) -> Result<CertificateDer<'static>> {
        let cert = self.leaf_params(hosts)?.signed_by(&self.leaf_key, &self.issuer).map_err(cert_err)?;
        Ok(cert.der().clone())
    }

    /// Issues a leaf with its own fresh key, as PEM `(certificate chain, private key)`. Handy for
    /// local test servers.
    pub fn issue_leaf_pem(&self, hosts: &[&str]) -> Result<(String, String)> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(cert_err)?;
        let cert = self.leaf_params(hosts)?.signed_by(&key, &self.issuer).map_err(cert_err)?;
        Ok((format!("{}{}", cert.pem(), self.cert_pem), key.serialize_pem()))
    }

    /// A rustls server config presenting a leaf for `host` (cached per host).
    pub(crate) fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>> {
        let key = host.to_ascii_lowercase();
        if let Some(c) = self.configs.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(c.clone());
        }
        let leaf = self.issue_leaf_der(&[&key])?;
        let certified = Arc::new(CertifiedKey::new(vec![leaf, self.cert_der.clone()], self.leaf_signer.clone()));
        let mut cfg = ServerConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| ProxyError::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(FixedCert(certified)));
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let cfg = Arc::new(cfg);
        let mut map = self.configs.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= MAX_CACHED_HOSTS {
            map.clear();
        }
        map.insert(key, cfg.clone());
        Ok(cfg)
    }
}

#[derive(Debug)]
struct FixedCert(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_reloads_the_same_ca() {
        let dir = tempfile::tempdir().unwrap();
        let ca = CertAuthority::load_or_create(dir.path()).unwrap();
        assert!(ca.common_name().starts_with("devall local CA ("));
        assert!(ca.common_name().len() <= 64);
        let info = ca.info();
        assert_eq!(info.sha256.len(), 32 * 3 - 1);
        assert_eq!(info.sha1.len(), 40);
        assert!(info.not_after - info.created_at >= (CA_VALIDITY_DAYS - 1) * 86_400_000);
        let again = CertAuthority::load_or_create(dir.path()).unwrap();
        assert_eq!(again.info().sha256, info.sha256);
        assert_eq!(again.spki_sha256(), ca.spki_sha256());
        // A leaf issued after reload still chains to the CA (same issuer DN and key).
        let leaf = again.issue_leaf_der(&["example.com"]).unwrap();
        verify_chain(&ca, &leaf, "example.com");
        let ip_leaf = again.issue_leaf_der(&["127.0.0.1"]).unwrap();
        verify_chain(&ca, &ip_leaf, "127.0.0.1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(CA_KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn refuses_project_folders() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("devall.yaml"), "id: x\nname: x\n").unwrap();
        let err = CertAuthority::create(dir.path().join(".devall").join("ca"), "x").unwrap_err();
        assert!(err.to_string().contains("inside the project"), "{err}");
    }

    #[test]
    fn caches_server_configs() {
        let dir = tempfile::tempdir().unwrap();
        let ca = CertAuthority::load_or_create(dir.path()).unwrap();
        let a = ca.server_config("Example.com").unwrap();
        let b = ca.server_config("example.com").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    fn verify_chain(ca: &CertAuthority, leaf: &CertificateDer<'static>, host: &str) {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.cert_der().clone()).unwrap();
        let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider()).build().unwrap();
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).unwrap();
        use rustls::client::danger::ServerCertVerifier;
        verifier.verify_server_cert(leaf, &[], &name, &[], rustls::pki_types::UnixTime::now()).unwrap();
    }
}
