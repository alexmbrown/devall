//! Privacy filter applied to every prompt before it leaves the app.
//!
//! * **Always** (local and remote providers): the values of the project's secrets — keychain
//!   entries referenced by environments or requests, plus every `.env`/`.env.local` value — are
//!   masked with [`devall_core::vars::redact_values`].
//! * **Remote providers** additionally get credential-pattern redaction: `Authorization`,
//!   `Cookie`, `X-Api-Key`… header values, `Bearer` tokens, JWTs, private-key blocks, URL
//!   passwords, `password=`/`token:`/`api_key=` style assignments and well-known token formats
//!   (OpenAI/Anthropic `sk-`, GitHub, Slack, AWS, Google, Stripe, Hugging Face, npm, GitLab).
//!
//! Template references such as `{{secret.STRIPE_KEY}}` are names, not values, and are kept.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use devall_core::secrets::{SecretResolver, load_dotenv};
use devall_core::vars::{SecretLookup, redact_values, references};
use devall_core::{Project, SecretStore};
use regex::Regex;

use crate::provider::ChatRequest;

pub const MASK: &str = "••••••";

/// Secret values to mask, gathered once per call.
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    values: BTreeSet<String>,
}

impl Redactor {
    pub fn new(values: BTreeSet<String>) -> Self {
        Self { values: values.into_iter().filter(|v| v.len() >= 4).collect() }
    }

    /// Every secret value the project could use (all environments + project-wide + `.env`).
    pub fn for_project(project: &Project, store: &dyn SecretStore) -> Self {
        Self::new(project_secret_values(project, store))
    }

    pub fn secret_count(&self) -> usize {
        self.values.len()
    }

    /// Masks secrets (always) and credential patterns (remote only). Returns the text and the
    /// number of replacements.
    pub fn redact(&self, text: &str, remote: bool) -> (String, usize) {
        let mut count: usize = self.values.iter().map(|v| text.matches(v.as_str()).count()).sum();
        let mut out = if count > 0 { redact_values(text, &self.values) } else { text.to_string() };
        if remote {
            let (o, n) = redact_credentials(&out);
            out = o;
            count += n;
        }
        (out, count)
    }

    /// Redacts the system prompt and every message in place.
    pub fn redact_request(&self, req: &mut ChatRequest, remote: bool) -> usize {
        let mut n = 0;
        if let Some(s) = req.system.as_mut() {
            let (o, c) = self.redact(s, remote);
            *s = o;
            n += c;
        }
        for m in &mut req.messages {
            let (o, c) = self.redact(&m.content, remote);
            m.content = o;
            n += c;
        }
        n
    }
}

/// Values of every secret the project references, looked up the same way requests resolve them
/// (keychain per environment, then project-wide, then `.env`). Values are only held in memory.
pub fn project_secret_values(project: &Project, store: &dyn SecretStore) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    let envs = project.environments().unwrap_or_default();
    for e in &envs {
        names.extend(e.file.secrets.iter().cloned());
    }
    if let Ok(tree) = project.tree() {
        for path in tree.request_paths() {
            if let Ok(req) = project.read_request(&path)
                && let Ok(text) = serde_json::to_string(&req)
            {
                names.extend(references(&text).into_iter().filter_map(|r| r.strip_prefix("secret.").map(|n| n.trim().to_string())));
            }
        }
    }
    let mut values = BTreeSet::new();
    let mut scopes: Vec<Option<&str>> = envs.iter().map(|e| Some(e.id.as_str())).collect();
    scopes.push(None);
    for env in scopes {
        let r = SecretResolver::new(store, project.id(), env, project.root());
        for n in &names {
            if let Some(v) = r.lookup(n).filter(|v| !v.is_empty()) {
                values.insert(v);
            }
        }
    }
    values.extend(load_dotenv(project.root()).into_values().filter(|v| v.len() >= 4));
    values
}

struct Pattern {
    re: Regex,
    /// Capture group holding the secret part (0 = whole match).
    group: usize,
}

fn patterns() -> &'static [Pattern] {
    static P: OnceLock<Vec<Pattern>> = OnceLock::new();
    P.get_or_init(|| {
        let p = |re: &str, group: usize| Pattern { re: Regex::new(re).expect("valid redaction regex"), group };
        vec![
            // PEM private keys (whole block).
            p(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----", 0),
            // Credential headers in HTTP dumps, JSON or YAML: keep the name, mask the value.
            p(
                r#"(?im)(\b(?:authorization|proxy-authorization|cookie|set-cookie|x-api-key|api-key|x-auth-token|x-access-token|x-amz-security-token|private-token)"?\s*[:=]\s*"?)([^\r\n"]+)"#,
                2,
            ),
            // Bearer / Basic tokens anywhere.
            p(r"(?i)\b(?:bearer|basic)\s+([A-Za-z0-9\-._~+/]{8,}=*)", 1),
            // JWTs.
            p(r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}", 0),
            // Well-known token formats.
            p(r"\b(?:sk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{22,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35}|(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{16,}|whsec_[A-Za-z0-9+/=]{16,}|glpat-[A-Za-z0-9_-]{20,}|hf_[A-Za-z0-9]{30,}|npm_[A-Za-z0-9]{36})\b", 0),
            // user:password@ in URLs.
            p(r"(?i)\b[a-z][a-z0-9+.-]*://[^/\s:@]+:([^@\s/]+)@", 1),
            // password=…, "client_secret": "…", api_key: … (values that are not {{templates}}).
            p(
                r#"(?i)\b[A-Za-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|client[_-]?secret|credentials?)"?\s*[:=]\s*"?([^\s"'{,;&}<>]{4,})"#,
                1,
            ),
        ]
    })
}

/// Pattern-based credential masking (see module docs). Returns the text and replacement count.
pub fn redact_credentials(text: &str) -> (String, usize) {
    let mut out = text.to_string();
    let mut total = 0;
    for pat in patterns() {
        let mut n = 0;
        let replaced = pat
            .re
            .replace_all(&out, |caps: &regex::Captures| {
                let whole = caps.get(0).unwrap();
                let Some(target) = caps.get(pat.group) else { return whole.as_str().to_string() };
                // Already masked, or a `{{template}}` reference (a name, not a value).
                if target.as_str().contains(MASK) || target.as_str().contains("{{") {
                    return whole.as_str().to_string();
                }
                n += 1;
                let start = target.start() - whole.start();
                let end = target.end() - whole.start();
                format!("{}{MASK}{}", &whole.as_str()[..start], &whole.as_str()[end..])
            })
            .into_owned();
        out = replaced;
        total += n;
    }
    (out, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devall_core::secrets::{ALL_ENVIRONMENTS, SecretScope};
    use devall_core::{EnvironmentFile, HttpRequestFile, KeyValue, MemorySecretStore};

    #[test]
    fn masks_credential_patterns() {
        let text = "GET /v1/users\nAuthorization: Bearer abc.def-123456789\nCookie: session=xyz123456\n\
            {\"api_key\": \"live-123456\", \"password\":\"hunter2!\"}\nurl https://bob:s3cretpw@db.example.com/x\n\
            token sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWX and AKIAABCDEFGHIJKLMNOP and \
            eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjMifQ.c2lnbmF0dXJlLXZhbHVl\n\
            ref {{secret.STRIPE_KEY}} stays; client_secret={{secret.X}}";
        let (out, n) = redact_credentials(text);
        for leaked in ["abc.def-123456789", "xyz123456", "live-123456", "hunter2!", "s3cretpw", "sk-ant-api03", "AKIAABCDEFGHIJKLMNOP", "eyJhbGciOiJIUzI1NiJ9"] {
            assert!(!out.contains(leaked), "{leaked} leaked:\n{out}");
        }
        assert!(out.contains("Authorization: ••••••"), "{out}");
        assert!(out.contains("https://bob:••••••@db.example.com"), "{out}");
        assert!(out.contains("{{secret.STRIPE_KEY}}") && out.contains("client_secret={{secret.X}}"), "{out}");
        assert!(out.contains("GET /v1/users"));
        assert!(n >= 8, "{n}");
        // Idempotent.
        assert_eq!(redact_credentials(&out).0, out);
    }

    #[test]
    fn secret_values_always_masked_patterns_only_remote() {
        let r = Redactor::new(["s3cr3t-value".to_string(), "abc".to_string()].into());
        let text = "token is s3cr3t-value; password=hunter22 abc";
        let (local, n) = r.redact(text, false);
        assert_eq!(local, "token is ••••••; password=hunter22 abc");
        assert_eq!(n, 1);
        let (remote, n) = r.redact(text, true);
        assert!(!remote.contains("hunter22") && !remote.contains("s3cr3t-value"), "{remote}");
        assert_eq!(n, 2);
    }

    #[test]
    fn gathers_project_secret_values_from_keychain_and_dotenv() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "Sec").unwrap();
        let mut env = EnvironmentFile::new("Staging");
        env.secrets = vec!["API_TOKEN".into()];
        p.save_environment("staging", &env).unwrap();
        let mut req = HttpRequestFile::new("Me");
        req.headers = vec![KeyValue::new("X-Other", "{{secret.OTHER}}")];
        p.create_request("", req).unwrap();
        std::fs::write(dir.path().join(".env"), "DB_PASSWORD=pg-pass-1234\n").unwrap();
        let store = MemorySecretStore::default();
        store.set(&SecretScope::new(p.id(), "staging"), "API_TOKEN", "tok-staging-999").unwrap();
        store.set(&SecretScope::new(p.id(), ALL_ENVIRONMENTS), "OTHER", "other-secret-42").unwrap();
        let values = project_secret_values(&p, &store);
        assert!(values.contains("tok-staging-999"));
        assert!(values.contains("other-secret-42"));
        assert!(values.contains("pg-pass-1234"));
        let r = Redactor::new(values);
        let (out, _) = r.redact("use tok-staging-999 and pg-pass-1234", false);
        assert_eq!(out, "use •••••• and ••••••");
    }
}
