//! `connections/<slug>.conn.yaml` (version 1). Field order is the key order on disk. Secret values
//! never appear here: passwords are templates such as `{{secret.DB_PASSWORD}}`, resolved with the
//! active environment when connecting (so switching dev → staging switches databases).

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

/// Unknown keys carried through a read → write round trip.
pub type Extra = IndexMap<String, serde_json::Value>;

fn format_version() -> u32 {
    FORMAT_VERSION
}
fn yes() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DbKind {
    Postgres,
    /// MySQL and MariaDB.
    Mysql,
    Sqlite,
    Redis,
    Mongodb,
    /// Microsoft SQL Server and Azure SQL (TDS).
    Sqlserver,
}

impl DbKind {
    pub fn default_port(self) -> Option<u16> {
        match self {
            DbKind::Postgres => Some(5432),
            DbKind::Mysql => Some(3306),
            DbKind::Redis => Some(6379),
            DbKind::Mongodb => Some(27017),
            DbKind::Sqlserver => Some(1433),
            DbKind::Sqlite => None,
        }
    }

    pub fn is_sql(self) -> bool {
        matches!(self, DbKind::Postgres | DbKind::Mysql | DbKind::Sqlite | DbKind::Sqlserver)
    }

    pub fn label(self) -> &'static str {
        match self {
            DbKind::Postgres => "PostgreSQL",
            DbKind::Mysql => "MySQL",
            DbKind::Sqlite => "SQLite",
            DbKind::Redis => "Redis",
            DbKind::Mongodb => "MongoDB",
            DbKind::Sqlserver => "SQL Server",
        }
    }
}

/// A port number, or a `{{template}}` that resolves to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PortSpec {
    Number(u16),
    Template(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    /// Plain TCP.
    Disable,
    /// TLS when the server offers it, without certificate checks (libpq's default).
    #[default]
    Prefer,
    /// TLS, without certificate checks.
    Require,
    /// TLS; the certificate must chain to the OS trust store (or `caCert`) and match the host.
    VerifyFull,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TlsConfig {
    #[serde(default)]
    pub mode: TlsMode,
    /// PEM file with the CA certificate(s) to trust instead of the OS store (relative to the
    /// project root, absolute, or `~/…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyCheck {
    /// Unknown hosts need a one-time confirmation in the app (trust on first use), after which
    /// the key is appended to `~/.ssh/known_hosts`. Changed keys are always rejected.
    #[default]
    Ask,
    /// Only hosts already in `~/.ssh/known_hosts` are accepted.
    Strict,
    /// No verification (insecure; for throwaway lab machines only).
    Off,
}

/// SSH tunnel (local port forwarding) to reach a database behind a bastion host.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SshConfig {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<PortSpec>,
    #[serde(default)]
    pub user: String,
    /// OpenSSH private key file (`~/.ssh/id_ed25519`, absolute, or project-relative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    /// Key passphrase, as a template (`{{secret.SSH_KEY_PASSPHRASE}}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
    /// Password, as a template (`{{secret.SSH_PASSWORD}}`). Without key and password the SSH
    /// agent is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default)]
    pub known_hosts_check: HostKeyCheck,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `connections/<slug>.conn.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    pub kind: DbKind,
    /// A full connection URL (templated), e.g. `postgres://app:{{secret.DB_PASSWORD}}@{{dbHost}}/app`
    /// or `mongodb+srv://…`. Fields set below override the URL's parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<PortSpec>,
    /// Database name; for SQLite the database file (project-relative or absolute, `:memory:`);
    /// for Redis the database index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Always a template such as `{{secret.DB_PASSWORD}}` — never a literal value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshConfig>,
    /// Sessions start read-only: mutating statements are rejected and the database session itself
    /// is read-only. The app can switch a session to read-write (with confirmation).
    #[serde(default = "yes")]
    pub read_only: bool,
    /// Production-like: switching to read-write requires typing the connection name.
    #[serde(default, skip_serializing_if = "is_false")]
    pub protected: bool,
    /// Colour chip in the UI (CSS colour or a token name such as `red`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Engine options (templated values), e.g. `connectTimeout: "10"`, `applicationName`,
    /// `statementTimeout` (ms, Postgres/MySQL/SQL Server), `sentinelMaster` (Redis), `authSource`
    /// (MongoDB), `instance` / `applicationIntent: ReadOnly` / `encrypt: strict` (SQL Server).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub options: IndexMap<String, String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl ConnectionFile {
    pub fn new(name: impl Into<String>, kind: DbKind) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: name.into(),
            kind,
            url: None,
            host: None,
            port: None,
            database: None,
            user: None,
            password: None,
            tls: None,
            ssh: None,
            read_only: true,
            protected: false,
            color: None,
            options: IndexMap::new(),
            extra: Extra::new(),
        }
    }

    /// A sensible starting point for a new connection of `kind`.
    pub fn starter(name: impl Into<String>, kind: DbKind) -> Self {
        let mut c = Self::new(name, kind);
        match kind {
            DbKind::Postgres => {
                c.host = Some("localhost".into());
                c.port = Some(PortSpec::Number(5432));
                c.database = Some("postgres".into());
                c.user = Some("postgres".into());
                c.password = Some("{{secret.DB_PASSWORD}}".into());
            }
            DbKind::Mysql => {
                c.host = Some("localhost".into());
                c.port = Some(PortSpec::Number(3306));
                c.user = Some("root".into());
                c.password = Some("{{secret.DB_PASSWORD}}".into());
            }
            DbKind::Sqlite => c.database = Some("data.db".into()),
            DbKind::Redis => {
                c.host = Some("localhost".into());
                c.port = Some(PortSpec::Number(6379));
            }
            DbKind::Mongodb => {
                c.host = Some("localhost".into());
                c.port = Some(PortSpec::Number(27017));
            }
            DbKind::Sqlserver => {
                c.host = Some("localhost".into());
                c.port = Some(PortSpec::Number(1433));
                c.database = Some("master".into());
                c.user = Some("sa".into());
                c.password = Some("{{secret.DB_PASSWORD}}".into());
            }
        }
        c
    }
}

/// JSON Schema of the `.conn.yaml` format (for editor validation; can be committed to
/// `schemas/connection.schema.json`).
pub fn json_schema() -> serde_json::Value {
    let mut v = schemars::schema_for!(ConnectionFile).to_value();
    if let Some(o) = v.as_object_mut() {
        o.insert("$id".into(), serde_json::Value::String(format!("{}/connection.schema.json", devall_core::schema::SCHEMA_BASE_URL)));
        o.insert("description".into(), serde_json::Value::String("connections/*.conn.yaml — a database connection (no secret values)".into()));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_and_defaults() {
        let text = r#"
version: 1
name: Orders DB
kind: postgres
host: "{{dbHost}}"
port: 5432
database: orders
user: app
password: "{{secret.DB_PASSWORD}}"
tls: { mode: verify-full, caCert: certs/ca.pem }
ssh: { host: bastion.example.com, user: deploy, privateKey: ~/.ssh/id_ed25519 }
protected: true
color: red
options: { applicationName: devall }
futureKey: kept
"#;
        let c: ConnectionFile = serde_yaml_ng::from_str(text).unwrap();
        assert!(c.read_only, "read-only by default");
        assert_eq!(c.tls.as_ref().unwrap().mode, TlsMode::VerifyFull);
        assert_eq!(c.ssh.as_ref().unwrap().known_hosts_check, HostKeyCheck::Ask);
        assert_eq!(c.port, Some(PortSpec::Number(5432)));
        assert_eq!(c.extra.get("futureKey").and_then(|v| v.as_str()), Some("kept"));
        let out = serde_yaml_ng::to_string(&c).unwrap();
        assert!(out.contains("readOnly: true"));
        assert!(out.contains("mode: verify-full"));
        assert!(out.contains("futureKey: kept"));
        let back: ConnectionFile = serde_yaml_ng::from_str(&out).unwrap();
        assert_eq!(back, c);
        let templ: ConnectionFile = serde_yaml_ng::from_str("name: x\nkind: redis\nport: '{{redisPort}}'\n").unwrap();
        assert_eq!(templ.port, Some(PortSpec::Template("{{redisPort}}".into())));
    }

    /// `schemas/connection.schema.json` is committed (copy it to the repo's `schemas/`); refresh with
    /// `UPDATE_SCHEMAS=1 cargo test -p devall-db schema_is_up_to_date`.
    #[test]
    fn schema_is_up_to_date() {
        let s = json_schema();
        assert!(s["$id"].as_str().unwrap().ends_with("connection.schema.json"));
        assert!(s["properties"].get("readOnly").is_some());
        let text = format!("{}\n", serde_json::to_string_pretty(&s).unwrap());
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas").join("connection.schema.json");
        if std::env::var_os("UPDATE_SCHEMAS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else {
            let committed = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
            assert_eq!(committed, text, "connection.schema.json is stale — run UPDATE_SCHEMAS=1 cargo test -p devall-db schema_is_up_to_date");
        }
    }
}
