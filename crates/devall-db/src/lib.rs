//! Database clients for /dev/all — the free, local alternative to TablePlus, Studio 3T and
//! DataGrip for everyday work:
//!
//! * [`sql`] — PostgreSQL (tokio-postgres), MySQL/MariaDB (mysql_async), SQLite (rusqlite) and
//!   SQL Server / Azure SQL (tiberius):
//!   schema introspection, multi-statement scripts with typed JSON results, row caps and paging,
//!   server-side cancel, EXPLAIN, parameterized row edits and batched inserts.
//! * [`redisdb`] — Redis/Valkey key browser, value viewers/editors, console, INFO/SLOWLOG,
//!   pub/sub.
//! * [`mongo`] — MongoDB find/aggregate/count/indexes/explain/writes with Extended JSON (and the
//!   mongo-shell dialect).
//! * [`ssh`] — SSH tunnels (russh) with known_hosts verification and trust-on-first-use.
//! * [`model`] / [`store`] — `connections/*.conn.yaml` and `queries/**` project files.
//! * [`resolve`] — `{{templates}}` and `{{secret.X}}` resolution per environment.
//! * [`fakeschema`] — fake-data generator schemas from the introspected tables or sampled
//!   collections (`devall-data` does the type/name mapping).
//! * [`session`] — an open connection with read-only state (read-only by default: mutating
//!   statements are rejected *and* the database session itself is read-only).
//!
//! Everything builds without system libraries (rustls with aws-lc everywhere, bundled SQLite).

pub mod error;
pub mod fakeschema;
pub mod model;
pub mod mongo;
pub mod redisdb;
pub mod resolve;
pub mod session;
pub mod sql;
pub mod ssh;
pub mod store;
pub mod tls;

pub use error::{DbError, Result};
pub use model::{ConnectionFile, DbKind, HostKeyCheck, PortSpec, SshConfig, TlsConfig, TlsMode, json_schema};
pub use resolve::{ResolvedConnection, resolve_connection};
pub use session::{ConnectOptions, Engine, SeedResult, Session, SessionInfo};
pub use sql::{Dialect, ExecOptions, QueryOutput, QueryProgress, SchemaTree, StatementResult, TableDetails};
pub use ssh::TunnelManager;
pub use store::{ConnectionEntry, QueryKind, SavedQuery, SavedQueryEntry};
