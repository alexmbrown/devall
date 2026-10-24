//! Project files owned by the database module:
//!
//! * `connections/**/<slug>.conn.yaml` — one connection per file ([`ConnectionFile`]).
//! * `queries/**/*.sql` (first lines may contain `-- connection: <slug>`), `*.redis`
//!   (`# connection: <slug>`), `*.mongo.json` (a JSON object with a `"connection"` key).
//!
//! Every `rel` path is `/`-separated and relative to the module's folder.

use std::fs;
use std::path::{Path, PathBuf};

use devall_core::fsutil::{atomic_write, normalize_rel, read_yaml, rel_join, safe_join, slugify, unique_name, write_yaml};
use serde::{Deserialize, Serialize};

use crate::error::{DbError, Result, invalid};
use crate::model::{ConnectionFile, DbKind, FORMAT_VERSION};

pub const CONN_DIR: &str = "connections";
pub const CONN_EXT: &str = ".conn.yaml";
pub const QUERY_DIR: &str = "queries";

/// Saved query files larger than this are not read (they are listed with an error).
const MAX_QUERY_BYTES: u64 = 8 * 1024 * 1024;

// ---------- connections ----------

/// A connection as listed in the UI (no secret values; templates are shown unresolved).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionEntry {
    /// Relative to `connections/`, e.g. `orders.conn.yaml`.
    pub path: String,
    /// The id saved queries refer to: the path without `.conn.yaml` (e.g. `orders`, `prod/orders`).
    pub slug: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<DbKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub read_only: bool,
    pub protected: bool,
    /// `host[:port]`, the SQLite file, or `url`, for display.
    pub target: String,
    pub ssh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn connections_dir(root: &Path) -> PathBuf {
    root.join(CONN_DIR)
}

fn conn_path(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = normalize_rel(rel);
    if !rel.ends_with(CONN_EXT) || rel.len() == CONN_EXT.len() {
        return Err(invalid(format!("'{rel}' is not a {CONN_EXT} file")));
    }
    Ok(safe_join(&connections_dir(root), &rel)?)
}

/// `orders.conn.yaml` → `orders`.
pub fn slug_of(rel: &str) -> String {
    normalize_rel(rel).trim_end_matches(CONN_EXT).to_string()
}

/// `orders` (or `orders.conn.yaml`) → `orders.conn.yaml`.
pub fn path_of_slug(slug: &str) -> String {
    let s = normalize_rel(slug);
    if s.ends_with(CONN_EXT) { s } else { format!("{s}{CONN_EXT}") }
}

fn display_target(c: &ConnectionFile) -> String {
    if c.kind == DbKind::Sqlite {
        return c.database.clone().or_else(|| c.url.clone()).unwrap_or_default();
    }
    match (&c.host, &c.port) {
        (Some(h), Some(crate::model::PortSpec::Number(p))) => format!("{h}:{p}"),
        (Some(h), Some(crate::model::PortSpec::Template(p))) => format!("{h}:{p}"),
        (Some(h), None) => h.clone(),
        // URLs may embed credentials; only show the scheme and what follows '@'.
        (None, _) => c.url.as_deref().map(strip_url_credentials).unwrap_or_default(),
    }
}

/// `postgres://user:pass@host/db` → `postgres://host/db`.
pub fn strip_url_credentials(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s => format!("{}{}", &url[..s + 3], &url[at + 1..]),
        _ => url.to_string(),
    }
}

/// Every connection under `connections/` (recursively), sorted by name. Unparseable files are
/// listed with `error` set so they can be fixed or deleted.
pub fn list_connections(root: &Path) -> Result<Vec<ConnectionEntry>> {
    let dir = connections_dir(root);
    let mut files = Vec::new();
    walk(&dir, "", &mut files, &|n| n.ends_with(CONN_EXT))?;
    let mut out = Vec::new();
    for rel in files {
        let path = dir.join(&rel);
        let entry = match read_yaml::<ConnectionFile>(&path) {
            Ok(c) => ConnectionEntry {
                slug: slug_of(&rel),
                name: c.name.clone(),
                kind: Some(c.kind),
                color: c.color.clone(),
                read_only: c.read_only,
                protected: c.protected,
                target: display_target(&c),
                ssh: c.ssh.is_some(),
                path: rel,
                error: None,
            },
            Err(e) => ConnectionEntry {
                slug: slug_of(&rel),
                name: slug_of(&rel),
                kind: None,
                color: None,
                read_only: true,
                protected: false,
                target: String::new(),
                ssh: false,
                path: rel,
                error: Some(e.to_string()),
            },
        };
        out.push(entry);
    }
    out.sort_by_key(|e| (e.name.to_lowercase(), e.path.clone()));
    Ok(out)
}

pub fn read_connection(root: &Path, rel: &str) -> Result<ConnectionFile> {
    let path = conn_path(root, rel)?;
    if !path.is_file() {
        return Err(DbError::NotFound(format!("{CONN_DIR}/{}", normalize_rel(rel))));
    }
    Ok(read_yaml(&path)?)
}

/// Writes a connection. Refuses literal passwords: they must be `{{…}}` templates (secrets).
pub fn save_connection(root: &Path, rel: &str, conn: &ConnectionFile) -> Result<()> {
    check_no_literal_secrets(conn)?;
    let mut c = conn.clone();
    c.version = FORMAT_VERSION;
    c.name = c.name.trim().to_string();
    if c.name.is_empty() {
        return Err(invalid("the connection needs a name"));
    }
    Ok(write_yaml(&conn_path(root, rel)?, &c)?)
}

/// Creates `connections/<slug(name)>.conn.yaml` (from `template` or a starter for `kind`); returns its path.
pub fn create_connection(root: &Path, name: &str, kind: DbKind, template: Option<ConnectionFile>) -> Result<String> {
    let dir = connections_dir(root);
    fs::create_dir_all(&dir).map_err(|e| DbError::Io(std::io::Error::new(e.kind(), format!("{}: {e}", dir.display()))))?;
    let file = unique_name(&dir, &slugify(name), CONN_EXT);
    let mut c = template.unwrap_or_else(|| ConnectionFile::starter(name, kind));
    c.name = name.trim().to_string();
    save_connection(root, &file, &c)?;
    Ok(file)
}

/// Moves the connection file to the OS trash.
pub fn delete_connection(root: &Path, rel: &str) -> Result<()> {
    let path = conn_path(root, rel)?;
    if !path.exists() {
        return Err(DbError::NotFound(format!("{CONN_DIR}/{}", normalize_rel(rel))));
    }
    trash::delete(&path).map_err(|e| DbError::Io(std::io::Error::other(format!("{}: {e}", path.display()))))
}

fn is_template_or_empty(v: &Option<String>) -> bool {
    match v.as_deref().map(str::trim) {
        None | Some("") => true,
        Some(s) => s.contains("{{") && s.contains("}}"),
    }
}

/// Passwords and passphrases must be templates; a URL must not embed a literal password.
pub fn check_no_literal_secrets(c: &ConnectionFile) -> Result<()> {
    let hint = "use a secret reference such as {{secret.DB_PASSWORD}} — values live in the OS keychain, never in project files";
    if !is_template_or_empty(&c.password) {
        return Err(invalid(format!("password must not be stored in the file: {hint}")));
    }
    if let Some(ssh) = &c.ssh {
        if !is_template_or_empty(&ssh.password) {
            return Err(invalid(format!("SSH password must not be stored in the file: {hint}")));
        }
        if !is_template_or_empty(&ssh.passphrase) {
            return Err(invalid(format!("SSH key passphrase must not be stored in the file: {hint}")));
        }
    }
    if let Some(url) = &c.url
        && let Some(pw) = url_password(url)
        && !(pw.contains("{{") && pw.contains("}}"))
    {
        return Err(invalid(format!("the URL contains a password: {hint}")));
    }
    Ok(())
}

/// The password part of `scheme://user:password@host…`, if any.
fn url_password(url: &str) -> Option<&str> {
    let rest = &url[url.find("://")? + 3..];
    let at = rest.rfind('@')?;
    let userinfo = &rest[..at];
    // Templates may contain ':' only inside braces; take the first ':' outside them.
    let mut depth = 0usize;
    for (i, c) in userinfo.bytes().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(&userinfo[i + 1..]).filter(|p| !p.is_empty()),
            _ => {}
        }
    }
    None
}

// ---------- saved queries ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueryKind {
    Sql,
    Redis,
    Mongo,
}

impl QueryKind {
    pub fn ext(self) -> &'static str {
        match self {
            QueryKind::Sql => ".sql",
            QueryKind::Redis => ".redis",
            QueryKind::Mongo => ".mongo.json",
        }
    }

    pub fn of(file_name: &str) -> Option<Self> {
        if file_name.ends_with(".mongo.json") {
            Some(QueryKind::Mongo)
        } else if file_name.ends_with(".sql") {
            Some(QueryKind::Sql)
        } else if file_name.ends_with(".redis") {
            Some(QueryKind::Redis)
        } else {
            None
        }
    }

    pub fn for_db(kind: DbKind) -> Self {
        match kind {
            DbKind::Redis => QueryKind::Redis,
            DbKind::Mongodb => QueryKind::Mongo,
            _ => QueryKind::Sql,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQueryEntry {
    /// Relative to `queries/`.
    pub path: String,
    /// File name without the extension.
    pub name: String,
    pub kind: QueryKind,
    /// Connection slug from the file header, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    pub path: String,
    pub kind: QueryKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    pub text: String,
}

pub fn queries_dir(root: &Path) -> PathBuf {
    root.join(QUERY_DIR)
}

fn query_path(root: &Path, rel: &str) -> Result<(PathBuf, QueryKind)> {
    let rel = normalize_rel(rel);
    let kind = QueryKind::of(&rel).ok_or_else(|| invalid(format!("'{rel}' is not a saved query (.sql, .redis, .mongo.json)")))?;
    Ok((safe_join(&queries_dir(root), &rel)?, kind))
}

pub fn list_queries(root: &Path) -> Result<Vec<SavedQueryEntry>> {
    let dir = queries_dir(root);
    let mut files = Vec::new();
    walk(&dir, "", &mut files, &|n| QueryKind::of(n).is_some())?;
    let mut out = Vec::new();
    for rel in files {
        let Some(kind) = QueryKind::of(&rel) else { continue };
        let name = devall_core::fsutil::rel_name(&rel).trim_end_matches(kind.ext()).to_string();
        let path = dir.join(&rel);
        let (connection, error) = match read_limited(&path) {
            Ok(text) => match kind {
                QueryKind::Mongo => match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) => (v.get("connection").and_then(|c| c.as_str()).map(str::to_string), None),
                    Err(e) => (None, Some(format!("invalid JSON: {e}"))),
                },
                _ => (header_connection(&text, kind), None),
            },
            Err(e) => (None, Some(e.to_string())),
        };
        out.push(SavedQueryEntry { path: rel, name, kind, connection, error });
    }
    out.sort_by_key(|e| e.path.to_lowercase());
    Ok(out)
}

fn read_limited(path: &Path) -> Result<String> {
    let meta = fs::metadata(path)?;
    if meta.len() > MAX_QUERY_BYTES {
        return Err(invalid(format!("{} is larger than 8 MB", path.display())));
    }
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
}

pub fn read_query(root: &Path, rel: &str) -> Result<SavedQuery> {
    let (path, kind) = query_path(root, rel)?;
    if !path.is_file() {
        return Err(DbError::NotFound(format!("{QUERY_DIR}/{}", normalize_rel(rel))));
    }
    let text = read_limited(&path)?;
    let connection = match kind {
        QueryKind::Mongo => serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("connection")?.as_str().map(str::to_string)),
        _ => header_connection(&text, kind),
    };
    Ok(SavedQuery { path: normalize_rel(rel), kind, connection, text })
}

pub fn save_query(root: &Path, rel: &str, text: &str) -> Result<()> {
    let (path, kind) = query_path(root, rel)?;
    if kind == QueryKind::Mongo {
        serde_json::from_str::<serde_json::Value>(text).map_err(|e| invalid(format!("not valid JSON: {e}")))?;
    }
    Ok(atomic_write(&path, text.as_bytes())?)
}

/// Creates `queries/<folder>/<slug(name)><ext>` with a connection header; returns its path.
pub fn create_query(root: &Path, folder: &str, name: &str, kind: QueryKind, connection: Option<&str>, text: Option<&str>) -> Result<String> {
    let folder = normalize_rel(folder);
    let dir = safe_join(&queries_dir(root), &folder)?;
    fs::create_dir_all(&dir)?;
    let file = unique_name(&dir, &slugify(name), kind.ext());
    let rel = rel_join(&folder, &file);
    let body = match kind {
        QueryKind::Mongo => {
            let mut v: serde_json::Value = match text {
                Some(t) if !t.trim().is_empty() => serde_json::from_str(t).map_err(|e| invalid(format!("not valid JSON: {e}")))?,
                _ => serde_json::json!({ "type": "find", "database": "", "collection": "", "filter": {}, "limit": 50 }),
            };
            if let (Some(conn), Some(o)) = (connection, v.as_object_mut()) {
                o.shift_insert(0, "connection".into(), serde_json::Value::String(conn.to_string()));
            }
            serde_json::to_string_pretty(&v).map_err(|e| invalid(e.to_string()))? + "\n"
        }
        _ => {
            let t = text.unwrap_or("");
            match connection {
                Some(c) => set_connection_header(t, kind, Some(c)),
                None => t.to_string(),
            }
        }
    };
    atomic_write(&safe_join(&queries_dir(root), &rel)?, body.as_bytes())?;
    Ok(rel)
}

pub fn delete_query(root: &Path, rel: &str) -> Result<()> {
    let (path, _) = query_path(root, rel)?;
    if !path.exists() {
        return Err(DbError::NotFound(format!("{QUERY_DIR}/{}", normalize_rel(rel))));
    }
    trash::delete(&path).map_err(|e| DbError::Io(std::io::Error::other(format!("{}: {e}", path.display()))))
}

fn comment_prefix(kind: QueryKind) -> &'static str {
    if kind == QueryKind::Redis { "#" } else { "--" }
}

/// The `connection:` value from the leading comment block of a `.sql` / `.redis` file.
pub fn header_connection(text: &str, kind: QueryKind) -> Option<String> {
    let prefix = comment_prefix(kind);
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Some(rest) = t.strip_prefix(prefix) else { break };
        let rest = rest.trim();
        if let Some(v) = rest.strip_prefix("connection:").or_else(|| rest.strip_prefix("connection =")) {
            let v = v.trim();
            return (!v.is_empty()).then(|| v.to_string());
        }
    }
    None
}

/// Sets (or removes, with `None`) the `-- connection: <slug>` header line.
pub fn set_connection_header(text: &str, kind: QueryKind, slug: Option<&str>) -> String {
    let prefix = comment_prefix(kind);
    let mut lines: Vec<&str> = text.lines().collect();
    let mut found = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Some(rest) = t.strip_prefix(prefix) else { break };
        if rest.trim().starts_with("connection:") {
            found = Some(i);
            break;
        }
    }
    let header = slug.map(|s| format!("{prefix} connection: {s}"));
    match (found, header.as_deref()) {
        (Some(i), Some(h)) => lines[i] = h,
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(h)) => lines.insert(0, h),
        (None, None) => {}
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') || out.is_empty() {
        out.push('\n');
    }
    out
}

// ---------- shared ----------

fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>, keep: &dyn Fn(&str) -> bool) -> Result<()> {
    let Ok(rd) = fs::read_dir(dir) else { return Ok(()) };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&entry.path(), &rel, out, keep)?;
        } else if keep(&name) {
            out.push(rel);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PortSpec;

    #[test]
    fn connection_crud() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(list_connections(root).unwrap().is_empty());
        let rel = create_connection(root, "Local Postgres", DbKind::Postgres, None).unwrap();
        assert_eq!(rel, "local-postgres.conn.yaml");
        let mut c = read_connection(root, &rel).unwrap();
        assert_eq!(c.port, Some(PortSpec::Number(5432)));
        assert!(c.read_only);
        c.color = Some("green".into());
        save_connection(root, &rel, &c).unwrap();
        let text = fs::read_to_string(root.join("connections").join(&rel)).unwrap();
        assert!(text.starts_with("version: 1\nname: Local Postgres\nkind: postgres\n"), "{text}");
        assert!(text.contains("password: '{{secret.DB_PASSWORD}}'"));
        fs::write(root.join("connections/broken.conn.yaml"), "name: [").unwrap();
        let list = list_connections(root).unwrap();
        assert_eq!(list.len(), 2);
        let ok = list.iter().find(|e| e.path == rel).unwrap();
        assert_eq!((ok.slug.as_str(), ok.target.as_str(), ok.kind), ("local-postgres", "localhost:5432", Some(DbKind::Postgres)));
        assert!(list.iter().any(|e| e.error.is_some()));
        assert!(read_connection(root, "../x.conn.yaml").is_err());
        assert!(read_connection(root, "x.yaml").is_err());
        assert_eq!(create_connection(root, "Local Postgres", DbKind::Postgres, None).unwrap(), "local-postgres-2.conn.yaml");
    }

    #[test]
    fn literal_passwords_are_refused() {
        let mut c = ConnectionFile::starter("x", DbKind::Postgres);
        c.password = Some("hunter2".into());
        assert!(check_no_literal_secrets(&c).is_err());
        c.password = Some("{{secret.PG}}".into());
        assert!(check_no_literal_secrets(&c).is_ok());
        c.url = Some("postgres://app:hunter2@db/app".into());
        assert!(check_no_literal_secrets(&c).is_err());
        c.url = Some("postgres://app:{{secret.PG}}@db/app".into());
        assert!(check_no_literal_secrets(&c).is_ok());
        c.url = Some("postgres://app@db/app".into());
        assert!(check_no_literal_secrets(&c).is_ok());
        assert_eq!(strip_url_credentials("redis://:pw@cache:6379/0"), "redis://cache:6379/0");
    }

    #[test]
    fn saved_queries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let rel = create_query(root, "reports", "Top customers", QueryKind::Sql, Some("orders"), Some("select 1;\n")).unwrap();
        assert_eq!(rel, "reports/top-customers.sql");
        let q = read_query(root, &rel).unwrap();
        assert_eq!(q.connection.as_deref(), Some("orders"));
        assert!(q.text.starts_with("-- connection: orders\nselect 1;"));
        let r = create_query(root, "", "keys", QueryKind::Redis, Some("cache"), Some("SCAN 0\n")).unwrap();
        assert_eq!(read_query(root, &r).unwrap().connection.as_deref(), Some("cache"));
        let m = create_query(root, "", "Big orders", QueryKind::Mongo, Some("shop"), None).unwrap();
        assert_eq!(m, "big-orders.mongo.json");
        assert_eq!(read_query(root, &m).unwrap().connection.as_deref(), Some("shop"));
        assert!(save_query(root, &m, "{not json").is_err());
        save_query(root, &rel, "-- connection: other\nselect 2;\n").unwrap();
        let list = list_queries(root).unwrap();
        assert_eq!(list.len(), 3);
        let sql = list.iter().find(|e| e.kind == QueryKind::Sql).unwrap();
        assert_eq!((sql.name.as_str(), sql.connection.as_deref()), ("top-customers", Some("other")));
        assert!(read_query(root, "../escape.sql").is_err());
        delete_query(root, &r).unwrap();
        assert_eq!(list_queries(root).unwrap().len(), 2);
    }

    #[test]
    fn connection_headers() {
        assert_eq!(header_connection("\n-- Report\n-- connection: pg-main\nselect 1", QueryKind::Sql).as_deref(), Some("pg-main"));
        assert_eq!(header_connection("select 1\n-- connection: x", QueryKind::Sql), None);
        assert_eq!(set_connection_header("select 1\n", QueryKind::Sql, Some("a")), "-- connection: a\nselect 1\n");
        assert_eq!(set_connection_header("-- connection: a\nselect 1\n", QueryKind::Sql, Some("b")), "-- connection: b\nselect 1\n");
        assert_eq!(set_connection_header("-- connection: a\nselect 1\n", QueryKind::Sql, None), "select 1\n");
        assert_eq!(set_connection_header("GET k", QueryKind::Redis, Some("c")), "# connection: c\nGET k");
    }
}
