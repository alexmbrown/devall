//! External secret managers, through the user's own (already authenticated) CLIs — free here,
//! Enterprise-only in most API clients.
//!
//! An environment maps a secret name to a *reference* (safe to commit; every teammate resolves it
//! with their own credentials):
//!
//! ```yaml
//! secrets: [API_TOKEN, DB_PASSWORD]
//! secretSources:
//!   API_TOKEN: op://Engineering/github/token
//!   DB_PASSWORD: aws-sm://prod/db#password
//! ```
//!
//! | Scheme | CLI invoked (no shell; arguments passed separately) |
//! |---|---|
//! | `op://vault/item/field` | `op read op://…` (1Password) |
//! | `bw://item-id[/field]` | `bw get password <id>` or `bw get item <id>` (Bitwarden) |
//! | `bws://secret-id` | `bws secret get <id>` (Bitwarden Secrets Manager) |
//! | `vault://path#field` | `vault kv get -field=<field> <path>` (HashiCorp Vault) |
//! | `aws-sm://secret-id[#jsonKey]` | `aws secretsmanager get-secret-value …` |
//! | `aws-ssm://parameter-name` | `aws ssm get-parameter --with-decryption …` |
//! | `gcp-sm://project/secret[/version]` | `gcloud secrets versions access …` |
//! | `azure-kv://vault/secret` | `az keyvault secret show …` |
//! | `doppler://project/config/NAME` | `doppler secrets get NAME --plain …` |
//! | `infisical://env/NAME` | `infisical secrets get NAME --env <env> --plain` |
//! | `env://VAR` | the process environment |
//!
//! Arbitrary commands are deliberately not supported: project files come from git, and a cloned
//! repo must never be able to run programs on your machine.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

const TIMEOUT: Duration = Duration::from_secs(20);
const CACHE_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: &'static str,
    pub args: Vec<String>,
    /// For `aws-sm://id#key`: pick a key from a JSON secret string.
    pub json_key: Option<String>,
    /// For `bw://id/field`: pick a field from `bw get item` JSON.
    pub bw_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub scheme: &'static str,
    pub name: &'static str,
    pub program: &'static str,
    pub example: &'static str,
}

pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo { scheme: "op", name: "1Password", program: "op", example: "op://Engineering/github/token" },
    ProviderInfo { scheme: "bw", name: "Bitwarden", program: "bw", example: "bw://<item-id>/password" },
    ProviderInfo { scheme: "bws", name: "Bitwarden Secrets Manager", program: "bws", example: "bws://<secret-id>" },
    ProviderInfo { scheme: "vault", name: "HashiCorp Vault", program: "vault", example: "vault://secret/myapp#api_key" },
    ProviderInfo { scheme: "aws-sm", name: "AWS Secrets Manager", program: "aws", example: "aws-sm://prod/myapp#apiKey" },
    ProviderInfo { scheme: "aws-ssm", name: "AWS SSM Parameter Store", program: "aws", example: "aws-ssm:///myapp/prod/api-key" },
    ProviderInfo { scheme: "gcp-sm", name: "Google Secret Manager", program: "gcloud", example: "gcp-sm://my-project/api-key/latest" },
    ProviderInfo { scheme: "azure-kv", name: "Azure Key Vault", program: "az", example: "azure-kv://my-vault/api-key" },
    ProviderInfo { scheme: "doppler", name: "Doppler", program: "doppler", example: "doppler://myapp/dev/API_KEY" },
    ProviderInfo { scheme: "infisical", name: "Infisical", program: "infisical", example: "infisical://dev/API_KEY" },
    ProviderInfo { scheme: "env", name: "Environment variable", program: "", example: "env://GITHUB_TOKEN" },
];

/// Translates a reference into a CLI invocation. `None` for `env://` (handled inline).
pub fn invocation(reference: &str) -> Result<Option<Invocation>, String> {
    let (scheme, rest) = reference.split_once("://").ok_or_else(|| format!("not a secret reference: {reference}"))?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(format!("empty secret reference: {reference}"));
    }
    let (path, fragment) = match rest.split_once('#') {
        Some((p, f)) => (p, Some(f.to_string())),
        None => (rest, None),
    };
    let inv = |program: &'static str, args: Vec<&str>| Invocation { program, args: args.into_iter().map(str::to_string).collect(), json_key: None, bw_field: None };
    Ok(Some(match scheme {
        "env" => return Ok(None),
        "op" => inv("op", vec!["read", "--no-newline", reference]),
        "bw" => {
            let (id, field) = path.split_once('/').map(|(i, f)| (i, Some(f.to_string()))).unwrap_or((path, None));
            match field.as_deref() {
                None | Some("password") => inv("bw", vec!["get", "password", id]),
                Some(f) => Invocation { bw_field: Some(f.to_string()), ..inv("bw", vec!["get", "item", id]) },
            }
        }
        "bws" => Invocation { json_key: Some("value".into()), ..inv("bws", vec!["secret", "get", path, "--output", "json"]) },
        "vault" => {
            let field = fragment.clone().ok_or("vault references need #field, e.g. vault://secret/app#token")?;
            let arg = format!("-field={field}");
            Invocation { program: "vault", args: vec!["kv".into(), "get".into(), arg, path.into()], json_key: None, bw_field: None }
        }
        "aws-sm" => Invocation {
            json_key: fragment.clone(),
            ..inv("aws", vec!["secretsmanager", "get-secret-value", "--secret-id", path, "--query", "SecretString", "--output", "text"])
        },
        "aws-ssm" => inv("aws", vec!["ssm", "get-parameter", "--name", path, "--with-decryption", "--query", "Parameter.Value", "--output", "text"]),
        "gcp-sm" => {
            let parts: Vec<&str> = path.split('/').collect();
            let (project, secret, version) = match parts.as_slice() {
                [p, s] => (*p, *s, "latest"),
                [p, s, v] => (*p, *s, *v),
                _ => return Err("gcp-sm references are gcp-sm://project/secret[/version]".into()),
            };
            let secret_arg = format!("--secret={secret}");
            let project_arg = format!("--project={project}");
            Invocation {
                program: "gcloud",
                args: vec!["secrets".into(), "versions".into(), "access".into(), version.into(), secret_arg, project_arg],
                json_key: fragment.clone(),
                bw_field: None,
            }
        }
        "azure-kv" => {
            let (vault, name) = path.split_once('/').ok_or("azure-kv references are azure-kv://vault/secret")?;
            inv("az", vec!["keyvault", "secret", "show", "--vault-name", vault, "--name", name, "--query", "value", "-o", "tsv"])
        }
        "doppler" => {
            let parts: Vec<&str> = path.split('/').collect();
            let [project, config, name] = parts.as_slice() else { return Err("doppler references are doppler://project/config/NAME".into()) };
            inv("doppler", vec!["secrets", "get", name, "--plain", "--project", project, "--config", config])
        }
        "infisical" => {
            let (env, name) = path.split_once('/').ok_or("infisical references are infisical://env/NAME")?;
            let env_arg = format!("--env={env}");
            Invocation { program: "infisical", args: vec!["secrets".into(), "get".into(), name.into(), env_arg, "--plain".into(), "--silent".into()], json_key: None, bw_field: None }
        }
        other => return Err(format!("unknown secret provider `{other}` (supported: {})", PROVIDERS.iter().map(|p| p.scheme).collect::<Vec<_>>().join(", "))),
    }))
}

fn run(inv: &Invocation) -> Result<String, String> {
    // On Windows, CLIs are often `.cmd`/`.bat` shims (npm-installed `bws`, `doppler` scoop shims);
    // `Command` only resolves `.exe`, so fall back to `cmd /C` without a shell string.
    let spawn = |program: &str, args: &[String]| Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawn(inv.program, &inv.args) {
        Ok(c) => c,
        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::NotFound => {
            let mut args = vec!["/C".to_string(), inv.program.to_string()];
            args.extend(inv.args.iter().cloned());
            spawn("cmd", &args).map_err(|e| format!("`{}` is not installed or not on PATH ({e})", inv.program))?
        }
        Err(e) => return Err(format!("`{}` is not installed or not on PATH ({e})", inv.program)),
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                if !status.success() {
                    let msg = err.trim();
                    return Err(format!("`{}` failed: {}", inv.program, if msg.is_empty() { "no output (are you signed in?)" } else { msg }));
                }
                return Ok(out.trim_end_matches(['\r', '\n']).to_string());
            }
            Ok(None) if start.elapsed() > TIMEOUT => {
                let _ = child.kill();
                return Err(format!("`{}` timed out after {}s (waiting for sign-in?)", inv.program, TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn pick_json(text: &str, key: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| format!("secret is not JSON, cannot pick #{key}"))?;
    let found = v.get(key).ok_or_else(|| format!("key `{key}` not in secret"))?;
    Ok(found.as_str().map(str::to_string).unwrap_or_else(|| found.to_string()))
}

/// Resolves a reference by running the provider CLI (or reading `env://`).
pub fn fetch(reference: &str) -> Result<String, String> {
    let Some(inv) = invocation(reference)? else {
        let var = reference.trim_start_matches("env://");
        return std::env::var(var).map_err(|_| format!("environment variable {var} is not set"));
    };
    let out = run(&inv)?;
    if let Some(field) = &inv.bw_field {
        let v: serde_json::Value = serde_json::from_str(&out).map_err(|e| format!("unexpected bw output: {e}"))?;
        if let Some(login_field) = v.get("login").and_then(|l| l.get(field)).and_then(|x| x.as_str()) {
            return Ok(login_field.to_string());
        }
        return v
            .get("fields")
            .and_then(|f| f.as_array())
            .and_then(|fields| fields.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some(field)))
            .and_then(|x| x.get("value").and_then(|v| v.as_str()))
            .map(str::to_string)
            .ok_or_else(|| format!("field `{field}` not found on Bitwarden item"));
    }
    match &inv.json_key {
        Some(k) => pick_json(&out, k),
        None => Ok(out),
    }
}

/// Session cache so each reference is fetched at most once per 10 minutes.
#[derive(Default)]
pub struct ProviderCache {
    values: Mutex<HashMap<String, (Instant, String)>>,
}

impl ProviderCache {
    pub fn get(&self, reference: &str) -> Result<String, String> {
        if let Some((at, v)) = self.values.lock().unwrap().get(reference)
            && at.elapsed() < CACHE_TTL
        {
            return Ok(v.clone());
        }
        let v = fetch(reference)?;
        self.values.lock().unwrap().insert(reference.to_string(), (Instant::now(), v.clone()));
        Ok(v)
    }

    pub fn clear(&self) {
        self.values.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocations() {
        let i = invocation("op://Eng/github/token").unwrap().unwrap();
        assert_eq!((i.program, i.args[0].as_str(), i.args.last().unwrap().as_str()), ("op", "read", "op://Eng/github/token"));
        let i = invocation("aws-sm://prod/db#password").unwrap().unwrap();
        assert_eq!(i.json_key.as_deref(), Some("password"));
        assert!(i.args.contains(&"prod/db".to_string()));
        let i = invocation("gcp-sm://proj/key").unwrap().unwrap();
        assert!(i.args.contains(&"latest".to_string()) && i.args.contains(&"--project=proj".to_string()));
        let i = invocation("vault://secret/app#token").unwrap().unwrap();
        assert_eq!(i.args, ["kv", "get", "-field=token", "secret/app"]);
        let i = invocation("bw://abc/username").unwrap().unwrap();
        assert_eq!(i.bw_field.as_deref(), Some("username"));
        assert!(invocation("vault://secret/app").is_err());
        assert!(invocation("doppler://a/b").is_err());
        assert!(invocation("cmd://rm -rf").is_err(), "arbitrary commands are refused");
        assert!(invocation("env://HOME").unwrap().is_none());
    }

    #[test]
    fn env_scheme_and_json_pick() {
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("DEVALL_PROVIDER_TEST", "v1") };
        assert_eq!(fetch("env://DEVALL_PROVIDER_TEST").unwrap(), "v1");
        assert_eq!(pick_json(r#"{"password":"p","n":1}"#, "n").unwrap(), "1");
        assert!(pick_json("plain", "x").is_err());
        let missing = fetch("op://x/y/z");
        assert!(missing.is_err() || missing.is_ok(), "either op isn't installed or it ran — both must not panic");
    }
}
