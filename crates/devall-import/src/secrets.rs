//! Credential detection. Imported variables that look like credentials become secret references
//! (`{{secret.NAME}}`) whose values are handed back to the caller — never written to files.
//! Literal credentials inside requests (auth fields, `Authorization`/API-key headers and params)
//! are lifted out the same way.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::Serialize;

use devall_core::{Auth, FolderFile, HttpRequestFile, KeyValue};

use crate::plan::ImportPlan;
use crate::util::{map_templates, safe_var_name, upper_snake, visit_folder_strings, visit_request_strings};

/// A credential found while importing. The value is meant for the OS keychain only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedSecret {
    /// Environment id the value belongs to, or `*` for every environment.
    pub environment: String,
    pub name: String,
    /// May be empty when the export only declared the secret (Bruno, OpenCollection).
    pub value: String,
    /// Where it was found, e.g. `environment “Staging”` or `auth of “Login”`.
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Index into `ImportPlan::environments`.
    Env(usize),
    All,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingSecret {
    pub scope: Scope,
    pub name: String,
    pub value: String,
    pub source: String,
}

/// Exact (normalized) variable names that are credentials.
const SECRET_NAMES: &[&str] = &["key", "pass", "pwd", "pin", "auth", "jwt", "otp"];
/// Substrings of normalized names that mark credentials.
const SECRET_PARTS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "apikey",
    "accesskey",
    "secretkey",
    "privatekey",
    "clientkey",
    "subscriptionkey",
    "masterkey",
    "signingkey",
    "encryptionkey",
    "licensekey",
    "credential",
    "authorization",
    "bearer",
    "session",
    "cookie",
    "signature",
];
/// Normalized name suffixes that mean "metadata about a credential", not the credential.
const NOT_SECRET_SUFFIXES: &[&str] = &[
    "url", "uri", "endpoint", "host", "path", "id", "type", "name", "header", "prefix", "expiry", "expires", "expiresat", "expiresin", "ttl",
    "length", "location", "placement", "mode", "format", "source", "field", "param", "count", "enabled", "lifetime", "duration",
];

fn normalize(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

/// Whether a variable/header/param name designates a credential.
pub fn is_secret_name(name: &str) -> bool {
    let n = normalize(name);
    if n.is_empty() || NOT_SECRET_SUFFIXES.iter().any(|s| n.ends_with(s) && n != *s) {
        return false;
    }
    SECRET_NAMES.contains(&n.as_str()) || SECRET_PARTS.iter().any(|p| n.contains(p))
}

/// Whether a value is recognizably a credential regardless of its name.
pub fn is_secret_value(value: &str) -> bool {
    let v = value.trim();
    if v.len() < 8 || v.contains("{{") || v.contains(char::is_whitespace) && !v.starts_with("Bearer ") {
        return v.contains("PRIVATE KEY-----");
    }
    let jwt = v.starts_with("eyJ") && v.matches('.').count() == 2 && v.len() > 30;
    let prefixed = [
        "sk_live_", "sk_test_", "rk_live_", "rk_test_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "xoxa-",
        "xapp-", "SG.", "sk-", "AIza", "Bearer ", "npm_", "pypi-", "shpat_", "sq0atp-", "AccountKey=",
    ]
    .iter()
    .any(|p| v.starts_with(p) && v.len() >= p.len() + 12);
    let aws = (v.starts_with("AKIA") || v.starts_with("ASIA")) && v.len() == 20 && v.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    jwt || prefixed || aws || v.contains("PRIVATE KEY-----")
}

/// The import heuristic: a credential-like name (any value except references and URLs), or a
/// credential-like value.
pub fn looks_like_secret(name: &str, value: &str) -> bool {
    let v = value.trim();
    if v.contains("{{") {
        return false;
    }
    if is_secret_name(name) && !(v.starts_with("http://") || v.starts_with("https://")) {
        return true;
    }
    is_secret_value(v)
}

/// Moves credential-like variables out of the plan and rewrites references to them. Returns the
/// values found (the plan no longer contains them).
pub(crate) fn extract(plan: &mut ImportPlan) -> Vec<PendingSecret> {
    let mut out: Vec<PendingSecret> = Vec::new();
    // variable name → secret name
    let mut renames: IndexMap<String, String> = IndexMap::new();
    for env in &plan.environments {
        for s in &env.secrets {
            renames.entry(s.clone()).or_insert_with(|| safe_var_name(s));
        }
        for (k, v) in &env.variables {
            if looks_like_secret(k, v) {
                renames.entry(k.clone()).or_insert_with(|| safe_var_name(k));
            }
        }
    }
    for (k, v) in &plan.project_variables {
        if looks_like_secret(k, v) {
            renames.entry(k.clone()).or_insert_with(|| safe_var_name(k));
        }
    }
    plan.visit_settings_mut(&mut |_, s| {
        for (k, v) in &s.variables {
            if looks_like_secret(k, v) {
                renames.entry(k.clone()).or_insert_with(|| safe_var_name(k));
            }
        }
    });

    // Move values.
    let mut all_scope: Vec<String> = Vec::new();
    for (i, env) in plan.environments.iter_mut().enumerate() {
        let declared: Vec<String> = std::mem::take(&mut env.secrets);
        let mut names: Vec<String> = Vec::new();
        for (var, secret) in &renames {
            if let Some(v) = env.variables.shift_remove(var) {
                out.push(PendingSecret { scope: Scope::Env(i), name: secret.clone(), value: v, source: format!("environment “{}”", env.name) });
                names.push(secret.clone());
            } else if declared.contains(var) {
                out.push(PendingSecret { scope: Scope::Env(i), name: secret.clone(), value: String::new(), source: format!("environment “{}”", env.name) });
                names.push(secret.clone());
            }
        }
        env.secrets = names;
    }
    for (var, secret) in &renames {
        if let Some(v) = plan.project_variables.shift_remove(var) {
            out.push(PendingSecret { scope: Scope::All, name: secret.clone(), value: v, source: "collection variable".into() });
            all_scope.push(secret.clone());
        }
    }
    let mut folder_found: Vec<PendingSecret> = Vec::new();
    plan.visit_settings_mut(&mut |label, s| {
        for (var, secret) in &renames {
            if let Some(v) = s.variables.shift_remove(var) {
                folder_found.push(PendingSecret { scope: Scope::All, name: secret.clone(), value: v, source: format!("variable of {}", quote_label(label)) });
            }
        }
    });
    all_scope.extend(folder_found.iter().map(|p| p.name.clone()));
    out.extend(folder_found);

    // Rewrite references everywhere.
    if !renames.is_empty() {
        let rewrite = |s: &mut String| {
            if s.contains("{{") {
                *s = map_templates(s, |k| renames.get(k).map(|n| format!("{{{{secret.{n}}}}}")));
            }
        };
        let mut rw = rewrite;
        plan.visit_requests_mut(&mut |r| visit_request_strings(r, &mut rw));
        plan.visit_settings_mut(&mut |_, s| visit_folder_strings(s, &mut rw));
        for env in &mut plan.environments {
            env.variables.values_mut().for_each(&mut rw);
        }
        plan.project_variables.values_mut().for_each(&mut rw);
    }

    // Lift literal credentials out of requests and folders.
    let mut used: HashSet<String> = renames.values().cloned().collect();
    let mut lifter = Lifter { by_value: HashMap::new(), used: &mut used, found: Vec::new() };
    plan.visit_settings_mut(&mut |label, s| lifter.folder(label, s));
    plan.visit_requests_mut(&mut |r| lifter.request(r));
    all_scope.extend(lifter.found.iter().map(|p| p.name.clone()));
    out.extend(lifter.found);

    // Secrets shared by every environment are listed on each so their status is visible.
    for env in &mut plan.environments {
        for n in &all_scope {
            if !env.secrets.contains(n) {
                env.secrets.push(n.clone());
            }
        }
    }
    out
}

fn quote_label(label: &str) -> String {
    if label == "collection" { "the collection".into() } else { format!("folder “{label}”") }
}

struct Lifter<'a> {
    by_value: HashMap<String, String>,
    used: &'a mut HashSet<String>,
    found: Vec<PendingSecret>,
}

impl Lifter<'_> {
    /// Replaces a literal credential with a `{{secret.X}}` reference (same value → same name).
    fn lift(&mut self, value: &mut String, base: &str, source: String) {
        let v = value.trim();
        if v.is_empty() || v.contains("{{") {
            return;
        }
        let name = match self.by_value.get(v) {
            Some(n) => n.clone(),
            None => {
                let base = upper_snake(base);
                let mut name = base.clone();
                let mut n = 2;
                while self.used.contains(&name) {
                    name = format!("{base}_{n}");
                    n += 1;
                }
                self.used.insert(name.clone());
                self.by_value.insert(v.to_string(), name.clone());
                self.found.push(PendingSecret { scope: Scope::All, name: name.clone(), value: v.to_string(), source });
                name
            }
        };
        *value = format!("{{{{secret.{name}}}}}");
    }

    fn auth(&mut self, auth: &mut Auth, owner: &str) {
        let src = |what: &str| format!("{what} of {owner}");
        match auth {
            Auth::Basic { password, .. } | Auth::Digest { password, .. } | Auth::Ntlm { password, .. } => self.lift(password, "PASSWORD", src("auth password")),
            Auth::Bearer { token, .. } => self.lift(token, "TOKEN", src("bearer token")),
            Auth::ApiKey { key, value, .. } => {
                let base = if key.trim().is_empty() { "API_KEY".to_string() } else { key.clone() };
                self.lift(value, &base, src("API key"));
            }
            Auth::OAuth2(c) => {
                self.lift(&mut c.client_secret, "CLIENT_SECRET", src("OAuth 2.0 client secret"));
                self.lift(&mut c.password, "PASSWORD", src("OAuth 2.0 password"));
            }
            Auth::AwsSigV4 { secret_access_key, session_token, .. } => {
                self.lift(secret_access_key, "AWS_SECRET_ACCESS_KEY", src("AWS secret key"));
                if let Some(t) = session_token {
                    self.lift(t, "AWS_SESSION_TOKEN", src("AWS session token"));
                }
            }
            _ => {}
        }
    }

    fn rows(&mut self, rows: &mut [KeyValue], what: &str, owner: &str) {
        for kv in rows {
            let lname = kv.name.to_ascii_lowercase();
            if lname == "authorization" || lname == "proxy-authorization" {
                // Keep the scheme readable: `Bearer {{secret.TOKEN}}`.
                let (scheme, rest) = match kv.value.split_once(' ') {
                    Some((s, r)) if ["bearer", "basic", "token", "apikey", "key", "sso-key"].contains(&s.to_ascii_lowercase().as_str()) => (Some(s.to_string()), r.to_string()),
                    _ => (None, kv.value.clone()),
                };
                let mut rest = rest;
                let base = match scheme.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    Some("basic") => "BASIC_CREDENTIALS",
                    Some(_) => "TOKEN",
                    None => "AUTHORIZATION",
                };
                self.lift(&mut rest, base, format!("{what} {} of {owner}", kv.name));
                kv.value = match scheme {
                    Some(s) => format!("{s} {rest}"),
                    None => rest,
                };
            } else if looks_like_secret(&kv.name, &kv.value) && !kv.value.trim().is_empty() {
                let name = kv.name.clone();
                self.lift(&mut kv.value, &name, format!("{what} {name} of {owner}"));
            }
        }
    }

    fn request(&mut self, r: &mut HttpRequestFile) {
        let owner = format!("“{}”", r.name);
        self.auth(&mut r.auth, &owner);
        self.rows(&mut r.headers, "header", &owner);
        self.rows(&mut r.params, "query parameter", &owner);
    }

    fn folder(&mut self, label: &str, s: &mut FolderFile) {
        let owner = quote_label(label);
        self.auth(&mut s.auth, &owner);
        self.rows(&mut s.headers, "header", &owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Format;
    use devall_core::EnvironmentFile;

    #[test]
    fn name_heuristics() {
        for n in ["password", "apiKey", "api_key", "X-API-Key", "accessToken", "client_secret", "token", "Authorization", "sessionCookie", "key", "AWS_SECRET_ACCESS_KEY"] {
            assert!(is_secret_name(n), "{n}");
        }
        for n in ["tokenUrl", "authUrl", "clientId", "keyId", "username", "baseUrl", "author", "sortKey", "tokenType", "passwordResetPath", "monkey"] {
            assert!(!is_secret_name(n), "{n}");
        }
    }

    #[test]
    fn value_heuristics() {
        assert!(is_secret_value("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop"));
        assert!(is_secret_value("ghp_abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(is_secret_value("sk_live_abcdefghijklmnop"));
        assert!(is_secret_value("AKIAIOSFODNN7EXAMPLE"));
        assert!(!is_secret_value("hello world"));
        assert!(!is_secret_value("https://api.example.com"));
        assert!(!looks_like_secret("token", "{{otherToken}}"));
        assert!(!looks_like_secret("tokenEndpoint", "https://x"));
        assert!(looks_like_secret("token", ""));
    }

    #[test]
    fn extracts_and_rewrites() {
        let mut plan = ImportPlan::new("x", Format::Postman);
        let mut env = EnvironmentFile::new("Staging");
        env.variables.insert("baseUrl".into(), "https://staging.example.com".into());
        env.variables.insert("apiToken".into(), "abc123".into());
        plan.environments.push(env);
        plan.project_variables.insert("clientSecret".into(), "s3cr3t".into());
        let mut req = HttpRequestFile::new("Login");
        req.url = "{{baseUrl}}/login".into();
        req.headers.push(KeyValue::new("X-Token", "{{apiToken}}"));
        req.auth = Auth::Basic { username: "bob".into(), password: "hunter2".into() };
        req.body = devall_core::Body::Json { content: "{\"secret\": \"{{ clientSecret }}\"}".into() };
        plan.items.push(crate::plan::PlanItem::Request(Box::new(req)));

        let found = extract(&mut plan);
        let names: Vec<_> = found.iter().map(|p| (p.name.as_str(), p.value.as_str())).collect();
        assert!(names.contains(&("apiToken", "abc123")), "{names:?}");
        assert!(names.contains(&("clientSecret", "s3cr3t")), "{names:?}");
        assert!(names.contains(&("PASSWORD", "hunter2")), "{names:?}");

        let env = &plan.environments[0];
        assert!(!env.variables.contains_key("apiToken"));
        assert!(env.secrets.contains(&"apiToken".to_string()));
        assert!(env.secrets.contains(&"PASSWORD".to_string()));
        assert!(plan.project_variables.is_empty());
        let (_, r) = &plan.requests()[0];
        assert_eq!(r.headers[0].value, "{{secret.apiToken}}");
        assert_eq!(r.auth, Auth::Basic { username: "bob".into(), password: "{{secret.PASSWORD}}".into() });
        assert!(matches!(&r.body, devall_core::Body::Json { content } if content.contains("{{secret.clientSecret}}")));
        assert_eq!(r.url, "{{baseUrl}}/login");
    }

    #[test]
    fn authorization_header_keeps_scheme() {
        let mut plan = ImportPlan::new("x", Format::Har);
        let mut a = HttpRequestFile::new("A");
        a.headers.push(KeyValue::new("Authorization", "Bearer abc.def.ghi"));
        let mut b = HttpRequestFile::new("B");
        b.headers.push(KeyValue::new("authorization", "Bearer abc.def.ghi"));
        plan.items.push(crate::plan::PlanItem::Request(Box::new(a)));
        plan.items.push(crate::plan::PlanItem::Request(Box::new(b)));
        let found = extract(&mut plan);
        assert_eq!(found.len(), 1, "same value → one secret");
        for (_, r) in plan.requests() {
            assert_eq!(r.headers[0].value, "Bearer {{secret.TOKEN}}");
        }
    }
}
