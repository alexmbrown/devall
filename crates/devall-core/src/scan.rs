//! Secret scanner: catches credentials pasted into files that get committed (requests,
//! environments, notes, mocks). Runs on save in the app and as `devall scan` / a git pre-commit
//! hook. `{{template}}` references are ignored — that's the right way to use secrets.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub rule: &'static str,
    pub description: &'static str,
    /// 1-based.
    pub line: usize,
    pub column: usize,
    /// The match with its middle masked, e.g. `ghp_ab…yz`.
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileFindings {
    pub path: String,
    pub findings: Vec<Finding>,
}

struct Rule {
    id: &'static str,
    description: &'static str,
    re: Regex,
    /// Capture group holding the secret (0 = whole match).
    group: usize,
}

fn rule(id: &'static str, description: &'static str, pattern: &str, group: usize) -> Rule {
    Rule { id, description, re: Regex::new(pattern).expect("valid scanner regex"), group }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule("private-key", "Private key", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----", 0),
        rule("aws-access-key", "AWS access key ID", r"\b(?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16}\b", 0),
        rule("aws-secret-key", "AWS secret access key", r#"(?i)aws.{0,20}?(?:secret|key).{0,20}?['":=\s]+([A-Za-z0-9/+=]{40})\b"#, 1),
        rule("github-token", "GitHub token", r"\b(?:gh[pousr]_[A-Za-z0-9]{36,255}|github_pat_[A-Za-z0-9_]{60,255})\b", 0),
        rule("gitlab-token", "GitLab token", r"\bglpat-[A-Za-z0-9_-]{20,}\b", 0),
        rule("slack-token", "Slack token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}\b", 0),
        rule("slack-webhook", "Slack webhook URL", r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+", 0),
        rule("stripe-live-key", "Stripe live key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{20,}\b", 0),
        rule("google-api-key", "Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b", 0),
        rule("openai-key", "OpenAI API key", r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}\b", 0),
        rule("anthropic-key", "Anthropic API key", r"\bsk-ant-[A-Za-z0-9_-]{32,}\b", 0),
        rule("twilio-key", "Twilio API key", r"\bSK[0-9a-fA-F]{32}\b", 0),
        rule("sendgrid-key", "SendGrid API key", r"\bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}\b", 0),
        rule("npm-token", "npm access token", r"\bnpm_[A-Za-z0-9]{36}\b", 0),
        rule("jwt", "JSON Web Token", r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b", 0),
        rule(
            "basic-auth-url",
            "Password in URL",
            r"\b[a-z][a-z0-9+.-]*://[^\s:/@{}]+:([^\s:/@{}]{3,})@[^\s/]+",
            1,
        ),
        rule(
            "generic-secret",
            "Credential-looking value",
            r#"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token|client[_-]?secret|private[_-]?key)\b["']?\s*[:=]\s*["']?([A-Za-z0-9_\-+/=.!@#$%^&*]{8,})"#,
            1,
        ),
    ]
});

/// Shannon entropy in bits per character.
fn entropy(s: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = s.chars().count() as f64;
    counts.values().map(|&c| {
        let p = c as f64 / n;
        -p * p.log2()
    }).sum()
}

fn mask(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return "•".repeat(chars.len());
    }
    format!("{}…{}", chars[..4].iter().collect::<String>(), chars[chars.len() - 2..].iter().collect::<String>())
}

/// Common placeholders that aren't secrets.
fn is_placeholder(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    v.contains("{{")
        || v.contains("${")
        || ["changeme", "password", "example", "placeholder", "your", "xxxx", "****", "••••", "redacted", "secret", "<", "dummy", "test1234"]
            .iter()
            .any(|p| l.contains(p))
}

/// Scans text; `{{template}}` references and obvious placeholders are ignored.
pub fn scan_text(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for r in RULES.iter() {
            for caps in r.re.captures_iter(line) {
                let Some(m) = caps.get(r.group).or_else(|| caps.get(0)) else { continue };
                let value = m.as_str();
                if is_placeholder(value) {
                    continue;
                }
                // Generic rule: require some randomness so `password: hunter` style words still flag
                // but `apiKey: header` (a field name) doesn't.
                if r.id == "generic-secret" && (entropy(value) < 3.0 || value.chars().all(|c| c.is_ascii_alphabetic())) {
                    continue;
                }
                if out.iter().any(|f: &Finding| f.line == i + 1 && f.column == m.start() + 1) {
                    continue;
                }
                out.push(Finding { rule: r.id, description: r.description, line: i + 1, column: m.start() + 1, excerpt: mask(value) });
            }
        }
    }
    out
}

/// Files a project commits that may contain pasted secrets.
fn scannable(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    (lower.ends_with(".yaml") || lower.ends_with(".yml") || lower.ends_with(".json") || lower.ends_with(".md") || lower.ends_with(".ipynb") || lower.ends_with(".http"))
        && !lower.starts_with(".devall/")
        && !lower.contains("/node_modules/")
}

/// Scans the project's tracked file types (http/, environments/, notes/, mocks/, devall.yaml…).
pub fn scan_project(root: &Path) -> Vec<FileFindings> {
    let mut out = Vec::new();
    fn walk(dir: &Path, rel: &str, out: &mut Vec<FileFindings>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let p = e.path();
            if p.is_dir() {
                walk(&p, &child, out);
            } else if scannable(&child)
                && e.metadata().map(|m| m.len() < 5 * 1024 * 1024).unwrap_or(false)
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                let findings = scan_text(&text);
                if !findings.is_empty() {
                    out.push(FileFindings { path: child, findings });
                }
            }
        }
    }
    walk(root, "", &mut out);
    out
}

/// A git pre-commit hook that runs `devall scan --staged`.
pub const PRE_COMMIT_HOOK: &str = "#!/bin/sh\n# Installed by /dev/all: blocks commits that contain credentials.\n# Bypass once with: git commit --no-verify\nexec devall scan --staged\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_real_looking_secrets() {
        let text = [
            "token: ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789",
            "aws: AKIAIOSFODNN7EXAMPLA",
            "url: https://admin:s3cr3tPass@db.internal:5432",
            "  password: \"Tr0ub4dor&3xq\"",
            "-----BEGIN RSA PRIVATE KEY-----",
            // Split so the fake key isn't a literal match for secret scanners on this repo.
            concat!("stripe: sk_", "live_51Habcdefghijklmnopqrstu"),
        ]
        .join("\n");
        let f = scan_text(&text);
        let rules: Vec<_> = f.iter().map(|x| x.rule).collect();
        for r in ["github-token", "aws-access-key", "basic-auth-url", "generic-secret", "private-key", "stripe-live-key"] {
            assert!(rules.contains(&r), "missing {r}: {rules:?}");
        }
        assert!(f.iter().all(|x| !x.excerpt.contains("aBcDeFgHiJ")), "excerpts are masked");
    }

    #[test]
    fn ignores_templates_and_placeholders() {
        let text = "token: '{{secret.API_TOKEN}}'\npassword: '{{secret.PASSWORD}}'\napiKey: header\npassword: changeme123\nurl: https://{{user}}:{{secret.PW}}@host";
        assert!(scan_text(text).is_empty(), "{:?}", scan_text(text));
    }

    #[test]
    fn scans_projects() {
        let dir = tempfile::tempdir().unwrap();
        let p = crate::Project::init(dir.path(), "S").unwrap();
        let mut req = crate::HttpRequestFile::new("Leaky");
        req.headers.push(crate::KeyValue::new("Authorization", "Bearer ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789"));
        p.create_request("", req).unwrap();
        std::fs::create_dir_all(dir.path().join(".devall")).unwrap();
        std::fs::write(dir.path().join(".devall/history.json"), "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789").unwrap();
        let found = scan_project(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "http/leaky.req.yaml");
    }
}
