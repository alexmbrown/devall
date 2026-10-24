//! Inbox files under `webhooks/` and routing of receiver paths to inboxes.

use std::fs;
use std::path::{Path, PathBuf};

use devall_core::fsutil::{read_yaml, slugify, unique_name, write_yaml};
use serde::Serialize;

use crate::WebhookError;
use crate::model::{FORMAT_VERSION, InboxFile, Issue, ProviderKind, validate};

pub const WEBHOOKS_DIR: &str = "webhooks";
pub const INBOX_EXT: &str = ".webhook.yaml";
pub const FIXTURES_DIR: &str = "fixtures";

/// One file in `webhooks/` (unparseable files carry `error`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxEntry {
    /// File stem: `stripe` for `webhooks/stripe.webhook.yaml`.
    pub id: String,
    pub file: Option<InboxFile>,
    pub error: Option<String>,
    pub issues: Vec<Issue>,
}

pub fn webhooks_dir(root: &Path) -> PathBuf {
    root.join(WEBHOOKS_DIR)
}

fn inbox_path(root: &Path, id: &str) -> Result<PathBuf, WebhookError> {
    if id.is_empty() || id.contains(['/', '\\', ':']) || id.starts_with('.') || id.contains("..") {
        return Err(WebhookError::Invalid(format!("invalid inbox id {id:?}")));
    }
    Ok(webhooks_dir(root).join(format!("{id}{INBOX_EXT}")))
}

/// Every `webhooks/*.webhook.yaml`, sorted by name. Duplicate paths are reported as issues.
pub fn list(root: &Path) -> Result<Vec<InboxEntry>, WebhookError> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(webhooks_dir(root)) else { return Ok(out) };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(INBOX_EXT) else { continue };
        if id.starts_with('.') {
            continue;
        }
        match read_yaml::<InboxFile>(&entry.path()) {
            Ok(f) => {
                let issues = validate(&f);
                out.push(InboxEntry { id: id.to_string(), file: Some(f), error: None, issues });
            }
            Err(e) => out.push(InboxEntry { id: id.to_string(), file: None, error: Some(e.to_string()), issues: vec![] }),
        }
    }
    out.sort_by(|a, b| {
        let key = |e: &InboxEntry| e.file.as_ref().map(|f| f.name.to_lowercase()).unwrap_or_else(|| e.id.clone());
        key(a).cmp(&key(b)).then_with(|| a.id.cmp(&b.id))
    });
    // Flag paths claimed by more than one inbox (the first in this order wins).
    for i in 0..out.len() {
        let Some(path) = out[i].file.as_ref().map(|f| normalize_path(&f.path)) else { continue };
        if let Some(first) = out[..i].iter().find(|e| e.file.as_ref().is_some_and(|f| normalize_path(&f.path) == path)) {
            let msg = format!("path {path} is already used by inbox {}", first.id);
            out[i].issues.push(Issue { field: "path".into(), message: msg });
        }
    }
    Ok(out)
}

pub fn read(root: &Path, id: &str) -> Result<InboxFile, WebhookError> {
    let p = inbox_path(root, id)?;
    if !p.is_file() {
        return Err(WebhookError::NotFound(format!("inbox {id}")));
    }
    Ok(read_yaml(&p)?)
}

/// Saves (even with issues, so drafts persist) and returns the validation issues.
pub fn save(root: &Path, id: &str, inbox: &InboxFile) -> Result<Vec<Issue>, WebhookError> {
    let mut f = inbox.clone();
    f.version = FORMAT_VERSION;
    write_yaml(&inbox_path(root, id)?, &f)?;
    Ok(validate(&f))
}

/// Creates `webhooks/<slug>.webhook.yaml` from the provider's template; the path is made unique.
pub fn create(root: &Path, name: &str, provider: ProviderKind) -> Result<String, WebhookError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(WebhookError::Invalid("name is required".into()));
    }
    let dir = webhooks_dir(root);
    fs::create_dir_all(&dir).map_err(|e| WebhookError::Io(format!("{}: {e}", dir.display())))?;
    let file = unique_name(&dir, &slugify(name), INBOX_EXT);
    let id = file.trim_end_matches(INBOX_EXT).to_string();
    let mut f = InboxFile::template(name, provider);
    let taken: Vec<String> = list(root)?.into_iter().filter_map(|e| e.file.map(|f| normalize_path(&f.path))).collect();
    let base = f.path.clone();
    let mut n = 2;
    while taken.contains(&normalize_path(&f.path)) {
        f.path = format!("{base}-{n}");
        n += 1;
    }
    save(root, &id, &f)?;
    Ok(id)
}

/// Moves the inbox file to the OS trash. Its events stay in history.
pub fn delete(root: &Path, id: &str) -> Result<(), WebhookError> {
    let p = inbox_path(root, id)?;
    if !p.exists() {
        return Err(WebhookError::NotFound(format!("inbox {id}")));
    }
    trash::delete(&p).map_err(|e| WebhookError::Io(format!("{}: {e}", p.display())))
}

/// `/stripe/` → `/stripe`; empty → `/`.
pub fn normalize_path(p: &str) -> String {
    let p = p.trim();
    let p = if p.starts_with('/') { p.to_string() } else { format!("/{p}") };
    let t = p.trim_end_matches('/');
    if t.is_empty() { "/".into() } else { t.to_string() }
}

/// Inboxes loaded for routing (valid files only).
#[derive(Debug, Clone, Default)]
pub struct Router {
    inboxes: Vec<(String, InboxFile)>,
}

impl Router {
    pub fn load(root: &Path) -> Self {
        let inboxes = list(root).unwrap_or_default().into_iter().filter_map(|e| e.file.map(|f| (e.id, f))).collect();
        Self { inboxes }
    }

    pub fn from_inboxes(inboxes: Vec<(String, InboxFile)>) -> Self {
        Self { inboxes }
    }

    pub fn get(&self, id: &str) -> Option<&InboxFile> {
        self.inboxes.iter().find(|(i, _)| i == id).map(|(_, f)| f)
    }

    pub fn inboxes(&self) -> &[(String, InboxFile)] {
        &self.inboxes
    }

    /// Exact path match first, then the longest inbox path that is a segment prefix
    /// (`/stripe` handles `/stripe/connect`). `None` = catch-all.
    pub fn route(&self, request_path: &str) -> Option<(&str, &InboxFile)> {
        let path = normalize_path(request_path);
        if let Some((id, f)) = self.inboxes.iter().find(|(_, f)| normalize_path(&f.path) == path) {
            return Some((id, f));
        }
        self.inboxes
            .iter()
            .filter(|(_, f)| {
                let p = normalize_path(&f.path);
                p != "/" && path.starts_with(&format!("{p}/"))
            })
            .max_by_key(|(_, f)| normalize_path(&f.path).len())
            .or_else(|| self.inboxes.iter().find(|(_, f)| normalize_path(&f.path) == "/"))
            .map(|(id, f)| (id.as_str(), f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_and_unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(list(root).unwrap().is_empty());
        let a = create(root, "Stripe", ProviderKind::Stripe).unwrap();
        let b = create(root, "Stripe", ProviderKind::Stripe).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("stripe", "stripe-2"));
        assert_eq!(read(root, &b).unwrap().path, "/stripe-2");
        let text = fs::read_to_string(root.join("webhooks/stripe.webhook.yaml")).unwrap();
        assert!(text.starts_with("version: 1\nname: Stripe\npath: /stripe\nprovider: stripe\n"), "{text}");

        let mut f = read(root, &b).unwrap();
        f.path = "/stripe/".into();
        save(root, &b, &f).unwrap();
        let entries = list(root).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].issues.iter().any(|i| i.message.contains("already used by inbox stripe")), "{:?}", entries[1].issues);

        fs::write(root.join("webhooks/broken.webhook.yaml"), "name: [").unwrap();
        assert!(list(root).unwrap().iter().any(|e| e.id == "broken" && e.error.is_some()));
        assert!(read(root, "../x").is_err());
        assert!(matches!(read(root, "missing"), Err(WebhookError::NotFound(_))));
        // (A successful delete moves the file to the OS trash, so only the error path is tested.)
        assert!(matches!(delete(root, "missing"), Err(WebhookError::NotFound(_))));
        assert!(delete(root, "../escape").is_err());
        assert_eq!(a, "stripe");
    }

    #[test]
    fn routing() {
        let mk = |id: &str, p: &str| (id.to_string(), InboxFile::new(id, p));
        let r = Router::from_inboxes(vec![mk("stripe", "/stripe"), mk("connect", "/stripe/connect/"), mk("gh", "/github")]);
        assert_eq!(r.route("/stripe").unwrap().0, "stripe");
        assert_eq!(r.route("/stripe/").unwrap().0, "stripe");
        assert_eq!(r.route("/stripe/other").unwrap().0, "stripe");
        assert_eq!(r.route("/stripe/connect/acct_1").unwrap().0, "connect");
        assert!(r.route("/stripes").is_none());
        assert!(r.route("/").is_none());
        let with_root = Router::from_inboxes(vec![mk("all", "/"), mk("gh", "/github")]);
        assert_eq!(with_root.route("/anything").unwrap().0, "all");
        assert_eq!(with_root.route("/github/x").unwrap().0, "gh");
    }
}
