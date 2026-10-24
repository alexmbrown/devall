//! Turns a [`ConnectionFile`] into concrete connection parameters by resolving its `{{templates}}`
//! with a [`devall_core::Resolver`] (environment → project variables, `{{secret.X}}` from the
//! keychain / secret managers), exactly like HTTP requests are resolved at send time.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use devall_core::Resolver;
use indexmap::IndexMap;

use crate::error::{DbError, Result, invalid};
use crate::model::{ConnectionFile, DbKind, HostKeyCheck, PortSpec, TlsMode};

/// Connection parameters with every template resolved. Holds secret values: never serialize it.
#[derive(Clone)]
pub struct ResolvedConnection {
    pub name: String,
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    /// The port came from the file or URL (not the engine default). SQL Server named instances
    /// without one are looked up through the SQL Browser service.
    pub port_explicit: bool,
    pub database: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    /// MongoDB only: the full URL (`mongodb://` or `mongodb+srv://`) when one was given.
    pub url: Option<String>,
    pub tls: TlsMode,
    /// Whether the TLS mode was chosen explicitly (otherwise engines pick their own default).
    pub tls_explicit: bool,
    pub ca_cert: Option<PathBuf>,
    pub ssh: Option<SshTarget>,
    pub read_only: bool,
    pub protected: bool,
    pub color: Option<String>,
    pub options: IndexMap<String, String>,
    /// Every secret value used (passwords, passphrases), for redacting errors and logs.
    pub secret_values: BTreeSet<String>,
}

impl std::fmt::Debug for ResolvedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedConnection")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "••••••"))
            .field("tls", &self.tls)
            .field("ssh", &self.ssh.as_ref().map(|s| format!("{}@{}:{}", s.user, s.host, s.port)))
            .field("read_only", &self.read_only)
            .finish()
    }
}

#[derive(Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    pub check: HostKeyCheck,
    /// `known_hosts` file; `None` = `~/.ssh/known_hosts`.
    pub known_hosts: Option<PathBuf>,
}

#[derive(Clone)]
pub enum SshAuth {
    Key { path: PathBuf, passphrase: Option<String> },
    Password(String),
    /// The running SSH agent (`SSH_AUTH_SOCK`, or the Windows OpenSSH agent pipe).
    Agent,
}

impl ResolvedConnection {
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str).filter(|v| !v.trim().is_empty())
    }

    /// `connectTimeout` option in seconds (default 10).
    pub fn connect_timeout(&self) -> std::time::Duration {
        let secs = self.option("connectTimeout").and_then(|v| v.trim().parse::<f64>().ok()).filter(|s| *s > 0.0).unwrap_or(10.0);
        std::time::Duration::from_secs_f64(secs.min(600.0))
    }

    /// Human-readable target (`host:port/db`, file, …) without credentials.
    pub fn display_target(&self) -> String {
        match self.kind {
            DbKind::Sqlite => self.database.clone().unwrap_or_default(),
            DbKind::Mongodb if self.url.is_some() => crate::store::strip_url_credentials(self.url.as_deref().unwrap_or("")),
            _ => match &self.database {
                Some(db) if !db.is_empty() => format!("{}:{}/{}", self.host, self.port, db),
                _ => format!("{}:{}", self.host, self.port),
            },
        }
    }
}

/// The user's home directory (`%USERPROFILE%` on Windows, `$HOME` elsewhere).
pub fn home_dir() -> Option<PathBuf> {
    let vars: &[&str] = if cfg!(windows) { &["USERPROFILE", "HOME"] } else { &["HOME", "USERPROFILE"] };
    vars.iter().find_map(std::env::var_os).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `~/x` → home-relative; relative → project-relative; absolute stays.
pub fn expand_path(p: &str, project_root: &Path) -> PathBuf {
    let p = p.trim();
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    if p == "~"
        && let Some(home) = home_dir()
    {
        return home;
    }
    let path = PathBuf::from(p);
    if path.is_absolute() { path } else { project_root.join(path) }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn parse_port(s: &str, what: &str) -> Result<u16> {
    s.trim().parse::<u16>().map_err(|_| invalid(format!("{what} must be a port number (got '{}')", s.trim())))
}

fn pct_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Parts of a connection URL.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UrlParts {
    pub scheme: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub query: Vec<(String, String)>,
    pub fragment: Option<String>,
}

/// Parses `scheme://[user[:password]@]host[:port][/database][?k=v…][#fragment]`.
pub fn parse_url(url: &str) -> Result<UrlParts> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://").ok_or_else(|| invalid("the URL must start with scheme:// (e.g. postgres://)"))?;
    let (rest, fragment) = match rest.split_once('#') {
        Some((r, f)) => (r, Some(f.to_string())),
        None => (rest, None),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((r, q)) => (r, q),
        None => (rest, ""),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        Some(ui) => match ui.split_once(':') {
            Some((u, p)) => (Some(pct_decode(u)), Some(pct_decode(p))),
            None => (Some(pct_decode(ui)), None),
        },
        None => (None, None),
    };
    if hostport.contains(',') {
        return Err(invalid("multi-host URLs are not supported; use a single host"));
    }
    let (host, port) = if let Some(stripped) = hostport.strip_prefix('[') {
        // [ipv6]:port
        let end = stripped.find(']').ok_or_else(|| invalid("unterminated [IPv6] host"))?;
        let host = &stripped[..end];
        let port = stripped[end + 1..].strip_prefix(':').map(|p| parse_port(p, "URL port")).transpose()?;
        (Some(host.to_string()), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() => (Some(pct_decode(h)), Some(parse_port(p, "URL port")?)),
            Some((h, _)) => (Some(pct_decode(h)), None),
            None => (Some(pct_decode(hostport)), None),
        }
    };
    let query = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (pct_decode(k), pct_decode(v)),
            None => (pct_decode(kv), String::new()),
        })
        .collect();
    Ok(UrlParts {
        scheme: scheme.to_ascii_lowercase(),
        host: host.filter(|h| !h.is_empty()),
        port,
        user: user.filter(|u| !u.is_empty()),
        password,
        database: Some(pct_decode(path)).filter(|p| !p.is_empty()),
        query,
        fragment,
    })
}

/// Resolves a URL template; references inside the `user:password@` part are percent-encoded so
/// secrets containing `@`, `:` or `/` keep the URL valid.
pub fn resolve_url_template(template: &str, r: &mut Resolver<'_>) -> String {
    let Some(s) = template.find("://") else { return r.resolve(template) };
    let after = &template[s + 3..];
    let auth_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let Some(at) = after[..auth_end].rfind('@') else { return r.resolve(template) };
    let userinfo = &after[..at];
    let mut out = r.resolve(&template[..s + 3]);
    let mut rest = userinfo;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else { break };
        let reference = &rest[start..start + end + 2];
        let value = r.resolve(reference);
        if value == reference {
            out.push_str(reference);
        } else {
            out.push_str(&percent_encoding::utf8_percent_encode(&value, USERINFO).to_string());
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out.push_str(&r.resolve(&after[at..]));
    out
}

/// `jdbc:sqlserver://host:1433;databaseName=app;encrypt=true` and
/// `sqlserver://host\SQLEXPRESS` → the `scheme://host:port?k=v` form [`parse_url`] reads.
fn normalize_sqlserver_url(u: &str) -> String {
    let u = u.trim();
    let u = u.strip_prefix("jdbc:").unwrap_or(u);
    let Some((scheme, rest)) = u.split_once("://") else { return u.to_string() };
    let (main, props) = match rest.split_once(';') {
        Some((m, p)) => (m, p),
        None => (rest, ""),
    };
    // A named instance in the authority: host\INSTANCE[:port].
    let (main, instance) = match main.find('\\') {
        Some(i) => {
            let after = &main[i + 1..];
            let end = after.find([':', '/', '?']).unwrap_or(after.len());
            (format!("{}{}", &main[..i], &after[end..]), Some(after[..end].to_string()))
        }
        None => (main.to_string(), None),
    };
    let mut extra: Vec<String> = props.split(';').map(str::trim).filter(|kv| !kv.is_empty()).map(str::to_string).collect();
    if let Some(i) = instance.filter(|i| !i.is_empty()) {
        extra.push(format!("instanceName={i}"));
    }
    if extra.is_empty() {
        return format!("{scheme}://{main}");
    }
    let sep = if main.contains('?') { '&' } else { '?' };
    format!("{scheme}://{main}{sep}{}", extra.join("&"))
}

/// Connection-string keys compared case- and space-insensitively (`Trust Server Certificate`).
fn sqlserver_url_key(k: &str) -> String {
    k.chars().filter(|c| !c.is_whitespace() && *c != '_').collect::<String>().to_ascii_lowercase()
}

/// Everything but RFC 3986 unreserved characters.
const USERINFO: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

fn tls_from_sslmode(v: &str) -> Option<TlsMode> {
    Some(match v.to_ascii_lowercase().replace('_', "-").as_str() {
        "disable" | "disabled" | "false" | "off" => TlsMode::Disable,
        "allow" | "prefer" | "preferred" => TlsMode::Prefer,
        "require" | "required" | "true" | "on" => TlsMode::Require,
        "verify-ca" | "verify-full" | "verify-identity" => TlsMode::VerifyFull,
        _ => return None,
    })
}

/// Resolves every template in `conn`. Fails with [`DbError::Unresolved`] listing the missing
/// references (names only — values are never part of errors).
pub fn resolve_connection(conn: &ConnectionFile, r: &mut Resolver<'_>, project_root: &Path) -> Result<ResolvedConnection> {
    let url = non_empty(conn.url.as_ref().map(|u| resolve_url_template(u, r)));
    let mut res = |s: &Option<String>| -> Option<String> { s.as_ref().map(|v| r.resolve(v)) };
    let host = non_empty(res(&conn.host));
    let port = match &conn.port {
        Some(PortSpec::Number(n)) => Some(PortSpec::Number(*n)),
        Some(PortSpec::Template(t)) => Some(PortSpec::Template(r.resolve(t))),
        None => None,
    };
    let mut res = |s: &Option<String>| -> Option<String> { s.as_ref().map(|v| r.resolve(v)) };
    let database = non_empty(res(&conn.database));
    let user = non_empty(res(&conn.user));
    let password = res(&conn.password).filter(|p| !p.is_empty());
    let ca_cert = conn.tls.as_ref().and_then(|t| res(&t.ca_cert)).and_then(|p| non_empty(Some(p)));
    let options: IndexMap<String, String> = conn.options.iter().map(|(k, v)| (k.clone(), r.resolve(v))).collect();
    let ssh = match &conn.ssh {
        Some(s) => {
            let host = r.resolve(&s.host);
            let port = match &s.port {
                Some(PortSpec::Number(n)) => *n,
                Some(PortSpec::Template(t)) => {
                    let t = r.resolve(t);
                    if t.contains("{{") { 22 } else { parse_port(&t, "SSH port")? }
                }
                None => 22,
            };
            let user = r.resolve(&s.user);
            let key = s.private_key.as_ref().map(|k| r.resolve(k)).filter(|k| !k.trim().is_empty());
            let passphrase = s.passphrase.as_ref().map(|p| r.resolve(p)).filter(|p| !p.is_empty());
            let pw = s.password.as_ref().map(|p| r.resolve(p)).filter(|p| !p.is_empty());
            let auth = match (key, pw) {
                (Some(k), _) => SshAuth::Key { path: expand_path(&k, project_root), passphrase },
                (None, Some(p)) => SshAuth::Password(p),
                (None, None) => SshAuth::Agent,
            };
            Some(SshTarget { host: host.trim().to_string(), port, user: user.trim().to_string(), auth, check: s.known_hosts_check, known_hosts: None })
        }
        None => None,
    };

    if !r.unresolved.is_empty() {
        return Err(DbError::Unresolved(r.unresolved.iter().cloned().collect()));
    }
    let mut secret_values = r.secret_values.clone();

    let mut out = ResolvedConnection {
        name: conn.name.clone(),
        kind: conn.kind,
        host: String::new(),
        port: conn.kind.default_port().unwrap_or(0),
        port_explicit: false,
        database: None,
        user: None,
        password: None,
        url: None,
        tls: TlsMode::Prefer,
        tls_explicit: false,
        ca_cert: ca_cert.map(|p| expand_path(&p, project_root)),
        ssh,
        read_only: conn.read_only,
        protected: conn.protected,
        color: conn.color.clone(),
        options: IndexMap::new(),
        secret_values: BTreeSet::new(),
    };

    // URL first, then explicit fields override its parts.
    if let Some(u) = &url {
        match conn.kind {
            DbKind::Sqlite => {
                let p = u.strip_prefix("sqlite://").or_else(|| u.strip_prefix("sqlite:")).or_else(|| u.strip_prefix("file:")).unwrap_or(u);
                out.database = Some(p.split('?').next().unwrap_or(p).to_string());
            }
            DbKind::Mongodb => {
                if !(u.starts_with("mongodb://") || u.starts_with("mongodb+srv://")) {
                    return Err(invalid("a MongoDB URL starts with mongodb:// or mongodb+srv://"));
                }
                let parts = parse_url(u).ok();
                if let Some(p) = &parts {
                    out.host = p.host.clone().unwrap_or_default();
                    out.port = p.port.unwrap_or(27017);
                    out.database = p.database.clone();
                    if let Some(pw) = &p.password {
                        secret_values.insert(pw.clone());
                        secret_values.insert(percent_encoding::utf8_percent_encode(pw, USERINFO).to_string());
                    }
                }
                out.url = Some(u.clone());
            }
            kind => {
                let normalized = if kind == DbKind::Sqlserver { normalize_sqlserver_url(u) } else { u.clone() };
                let p = parse_url(&normalized)?;
                let ok = match kind {
                    DbKind::Postgres => matches!(p.scheme.as_str(), "postgres" | "postgresql"),
                    DbKind::Mysql => matches!(p.scheme.as_str(), "mysql" | "mariadb"),
                    DbKind::Redis => matches!(p.scheme.as_str(), "redis" | "rediss" | "valkey" | "valkeys"),
                    DbKind::Sqlserver => matches!(p.scheme.as_str(), "sqlserver" | "mssql"),
                    _ => false,
                };
                if !ok {
                    return Err(invalid(format!("'{}://' URLs can't be used for a {} connection", p.scheme, kind.label())));
                }
                if let Some(h) = p.host {
                    out.host = h;
                }
                if let Some(port) = p.port {
                    out.port = port;
                    out.port_explicit = true;
                }
                out.user = p.user;
                out.password = p.password;
                out.database = p.database;
                if matches!(p.scheme.as_str(), "rediss" | "valkeys") {
                    out.tls = if p.fragment.as_deref() == Some("insecure") { TlsMode::Require } else { TlsMode::VerifyFull };
                    out.tls_explicit = true;
                }
                let mut trust_server_cert = None;
                for (k, v) in p.query {
                    if kind == DbKind::Sqlserver {
                        match sqlserver_url_key(&k).as_str() {
                            "encrypt" => {
                                let lv = v.to_ascii_lowercase();
                                match lv.as_str() {
                                    "disable" | "off" | "danger_plaintext" => out.tls = TlsMode::Disable,
                                    "false" | "no" | "optional" => out.tls = TlsMode::Prefer,
                                    "strict" => {
                                        out.tls = TlsMode::VerifyFull;
                                        out.options.insert("encrypt".into(), "strict".into());
                                    }
                                    _ => out.tls = TlsMode::VerifyFull,
                                }
                                out.tls_explicit = true;
                                continue;
                            }
                            "trustservercertificate" => {
                                trust_server_cert = Some(matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1"));
                                continue;
                            }
                            "database" | "databasename" | "initialcatalog" => {
                                out.database = Some(v).filter(|d| !d.is_empty());
                                continue;
                            }
                            "user" | "userid" | "uid" => {
                                out.user = Some(v).filter(|d| !d.is_empty());
                                continue;
                            }
                            "password" | "pwd" => {
                                out.password = Some(v).filter(|d| !d.is_empty());
                                continue;
                            }
                            "instance" | "instancename" => {
                                out.options.insert("instance".into(), v);
                                continue;
                            }
                            "applicationintent" => {
                                out.options.insert("applicationIntent".into(), v);
                                continue;
                            }
                            "applicationname" | "app" => {
                                out.options.insert("applicationName".into(), v);
                                continue;
                            }
                            "connecttimeout" | "logintimeout" | "connectiontimeout" => {
                                out.options.insert("connectTimeout".into(), v);
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match k.as_str() {
                        "sslmode" | "ssl-mode" | "ssl_mode" | "tls" | "ssl" => {
                            if let Some(m) = tls_from_sslmode(&v) {
                                out.tls = m;
                                out.tls_explicit = true;
                            }
                        }
                        "application_name" => {
                            out.options.insert("applicationName".into(), v);
                        }
                        "connect_timeout" => {
                            out.options.insert("connectTimeout".into(), v);
                        }
                        _ => {
                            out.options.insert(k, v);
                        }
                    }
                }
                // `trustServerCertificate=true` keeps encryption but skips certificate checks.
                if trust_server_cert == Some(true) && out.tls == TlsMode::VerifyFull {
                    out.tls = TlsMode::Require;
                }
                if trust_server_cert.is_some() && !out.tls_explicit {
                    out.tls = if trust_server_cert == Some(true) { TlsMode::Require } else { TlsMode::VerifyFull };
                    out.tls_explicit = true;
                }
            }
        }
    }
    if let Some(h) = host {
        out.host = h;
    }
    match port {
        Some(PortSpec::Number(n)) => {
            out.port = n;
            out.port_explicit = true;
        }
        Some(PortSpec::Template(t)) if !t.trim().is_empty() => {
            out.port = parse_port(&t, "port")?;
            out.port_explicit = true;
        }
        _ => {}
    }
    if database.is_some() {
        out.database = database;
    }
    if user.is_some() {
        out.user = user;
    }
    if password.is_some() {
        out.password = password;
    }
    if let Some(t) = &conn.tls {
        out.tls = t.mode;
        out.tls_explicit = true;
    }
    for (k, v) in options {
        out.options.insert(k, v);
    }
    if conn.kind == DbKind::Sqlserver
        && let Some((h, instance)) = out.host.clone().split_once('\\')
    {
        // `host\INSTANCE` (the SSMS / sqlcmd spelling of a named instance).
        out.host = h.to_string();
        if !instance.is_empty() && !out.options.contains_key("instance") {
            out.options.insert("instance".into(), instance.to_string());
        }
    }
    if out.host.is_empty() && conn.kind != DbKind::Sqlite && out.url.is_none() {
        out.host = "localhost".into();
    }
    if conn.kind == DbKind::Sqlite {
        let db = out.database.clone().ok_or_else(|| invalid("set the SQLite database file"))?;
        if db != ":memory:" {
            out.database = Some(expand_path(&db, project_root).to_string_lossy().into_owned());
        }
    }
    if let Some(pw) = &out.password {
        secret_values.insert(pw.clone());
        let enc = percent_encoding::utf8_percent_encode(pw, USERINFO).to_string();
        secret_values.insert(enc);
    }
    if let Some(ssh) = &out.ssh {
        match &ssh.auth {
            SshAuth::Password(p) => {
                secret_values.insert(p.clone());
            }
            SshAuth::Key { passphrase: Some(p), .. } => {
                secret_values.insert(p.clone());
            }
            _ => {}
        }
    }
    out.secret_values = secret_values;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SshConfig, TlsConfig};
    use devall_core::SecretLookup;

    struct Secrets;
    impl SecretLookup for Secrets {
        fn lookup(&self, name: &str) -> Option<String> {
            match name {
                "DB_PASSWORD" => Some("p@ss:w/rd".into()),
                "SSH_PW" => Some("ssh-secret".into()),
                _ => None,
            }
        }
    }

    fn resolver<'a>(s: &'a Secrets, vars: &[(&str, &str)]) -> Resolver<'a> {
        let mut r = Resolver::new(s);
        r.push_layer(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        r
    }

    #[test]
    fn fields_and_templates() {
        let s = Secrets;
        let mut c = ConnectionFile::starter("Orders", DbKind::Postgres);
        c.host = Some("{{dbHost}}".into());
        c.port = Some(PortSpec::Template("{{dbPort}}".into()));
        c.ssh = Some(SshConfig { host: "bastion".into(), user: "me".into(), password: Some("{{secret.SSH_PW}}".into()), ..Default::default() });
        let mut r = resolver(&s, &[("dbHost", "db.staging"), ("dbPort", "6543")]);
        let rc = resolve_connection(&c, &mut r, Path::new("/p")).unwrap();
        assert_eq!((rc.host.as_str(), rc.port, rc.password.as_deref()), ("db.staging", 6543, Some("p@ss:w/rd")));
        assert!(rc.secret_values.contains("p@ss:w/rd"));
        assert!(rc.secret_values.contains("ssh-secret"));
        assert!(matches!(rc.ssh.as_ref().unwrap().auth, SshAuth::Password(_)));
        assert_eq!(rc.ssh.as_ref().unwrap().port, 22);
        assert!(!format!("{rc:?}").contains("p@ss"), "Debug never prints secrets");
    }

    #[test]
    fn unresolved_references_are_reported_by_name() {
        let s = Secrets;
        let mut c = ConnectionFile::starter("x", DbKind::Mysql);
        c.password = Some("{{secret.MISSING}}".into());
        c.host = Some("{{nope}}".into());
        let mut r = resolver(&s, &[]);
        let err = resolve_connection(&c, &mut r, Path::new("/p")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("{{secret.MISSING}}") && msg.contains("{{nope}}"), "{msg}");
    }

    #[test]
    fn url_with_overrides() {
        let s = Secrets;
        let mut c = ConnectionFile::new("x", DbKind::Postgres);
        c.url = Some("postgresql://app:{{secret.DB_PASSWORD}}@db.example.com:5433/orders?sslmode=verify-full&application_name=me".into());
        c.database = Some("other".into());
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&c, &mut r, Path::new("/p")).unwrap();
        assert_eq!(rc.host, "db.example.com");
        assert_eq!(rc.port, 5433);
        assert_eq!(rc.user.as_deref(), Some("app"));
        assert_eq!(rc.password.as_deref(), Some("p@ss:w/rd"), "secrets in URLs are percent-encoded then decoded");
        assert_eq!(rc.database.as_deref(), Some("other"));
        assert_eq!(rc.tls, TlsMode::VerifyFull);
        assert_eq!(rc.option("applicationName"), Some("me"));
        c.tls = Some(TlsConfig { mode: TlsMode::Disable, ..Default::default() });
        let mut r = resolver(&s, &[]);
        assert_eq!(resolve_connection(&c, &mut r, Path::new("/p")).unwrap().tls, TlsMode::Disable);
        c.url = Some("mysql://x@h/db".into());
        let mut r = resolver(&s, &[]);
        assert!(resolve_connection(&c, &mut r, Path::new("/p")).is_err(), "scheme must match the kind");
    }

    #[test]
    fn url_parsing() {
        let p = parse_url("redis://:pw%40x@[::1]:6380/2?protocol=resp3#insecure").unwrap();
        assert_eq!(p.host.as_deref(), Some("::1"));
        assert_eq!(p.port, Some(6380));
        assert_eq!(p.password.as_deref(), Some("pw@x"));
        assert_eq!(p.database.as_deref(), Some("2"));
        assert_eq!(p.query, vec![("protocol".to_string(), "resp3".to_string())]);
        assert_eq!(p.fragment.as_deref(), Some("insecure"));
        assert!(parse_url("postgres://a,b/db").is_err());
        assert!(parse_url("no-scheme").is_err());
    }

    #[test]
    fn sqlite_paths() {
        let s = Secrets;
        let mut c = ConnectionFile::new("x", DbKind::Sqlite);
        c.database = Some("db/app.sqlite".into());
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&c, &mut r, Path::new("/proj")).unwrap();
        assert_eq!(PathBuf::from(rc.database.unwrap()), Path::new("/proj").join("db/app.sqlite"));
        c.database = Some(":memory:".into());
        let mut r = resolver(&s, &[]);
        assert_eq!(resolve_connection(&c, &mut r, Path::new("/proj")).unwrap().database.as_deref(), Some(":memory:"));
        c.database = None;
        c.url = Some("sqlite:///tmp/x.db".into());
        let mut r = resolver(&s, &[]);
        assert!(resolve_connection(&c, &mut r, Path::new("/proj")).unwrap().database.unwrap().ends_with("x.db"));
    }

    #[test]
    fn sqlserver_urls_and_instances() {
        let s = Secrets;
        let mut c = ConnectionFile::new("x", DbKind::Sqlserver);
        c.url = Some("jdbc:sqlserver://db.example.com:1444;databaseName=orders;encrypt=true;trustServerCertificate=true;applicationIntent=ReadOnly;user=app;password={{secret.DB_PASSWORD}}".into());
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&c, &mut r, Path::new("/p")).unwrap();
        assert_eq!((rc.host.as_str(), rc.port, rc.port_explicit), ("db.example.com", 1444, true));
        assert_eq!(rc.database.as_deref(), Some("orders"));
        assert_eq!(rc.user.as_deref(), Some("app"));
        assert_eq!(rc.password.as_deref(), Some("p@ss:w/rd"));
        assert_eq!(rc.tls, TlsMode::Require, "encrypt + trustServerCertificate = encrypted, unverified");
        assert_eq!(rc.option("applicationIntent"), Some("ReadOnly"));

        c.url = Some("sqlserver://sa@winbox\\SQLEXPRESS/app?encrypt=strict".into());
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&c, &mut r, Path::new("/p")).unwrap();
        assert_eq!((rc.host.as_str(), rc.port, rc.port_explicit), ("winbox", 1433, false));
        assert_eq!(rc.option("instance"), Some("SQLEXPRESS"));
        assert_eq!((rc.tls, rc.option("encrypt")), (TlsMode::VerifyFull, Some("strict")));

        let mut f = ConnectionFile::starter("y", DbKind::Sqlserver);
        f.host = Some("10.0.0.5\\PROD".into());
        f.port = None;
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&f, &mut r, Path::new("/p")).unwrap();
        assert_eq!((rc.host.as_str(), rc.option("instance"), rc.port_explicit), ("10.0.0.5", Some("PROD"), false));
        assert_eq!(normalize_sqlserver_url("mssql://h:1/db"), "mssql://h:1/db");
        c.url = Some("postgres://h/db".into());
        let mut r = resolver(&s, &[]);
        assert!(resolve_connection(&c, &mut r, Path::new("/p")).is_err());
    }

    #[test]
    fn mongo_url_kept() {
        let s = Secrets;
        let mut c = ConnectionFile::new("x", DbKind::Mongodb);
        c.url = Some("mongodb+srv://u:{{secret.DB_PASSWORD}}@cluster0.example.net/shop?retryWrites=true".into());
        let mut r = resolver(&s, &[]);
        let rc = resolve_connection(&c, &mut r, Path::new("/p")).unwrap();
        assert!(rc.url.as_deref().unwrap().starts_with("mongodb+srv://u:p%40ss%3Aw%2Frd@cluster0"), "{:?}", rc.url);
        assert!(rc.secret_values.contains("p@ss:w/rd"));
        assert_eq!(rc.display_target(), "mongodb+srv://cluster0.example.net/shop?retryWrites=true");
    }
}
