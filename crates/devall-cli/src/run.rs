//! `devall run`: the collection runner for terminals and CI. Runs a project's requests, or a
//! loose `.http` / `.rest` / `.hurl` file in memory (nothing is written).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;
use devall_core::secrets::SecretResolver;
use devall_core::vars::SecretLookup;
use devall_core::{MemorySecretStore, SecretStore, TreeNode, Variables};
use devall_http::oauth2::MemoryTokenStore;
use devall_http::runner::{self, RunContext, RunHooks, RunItemResult, RunOptions};
use devall_http::{ClientPool, HttpResponseData, PreparedRequest};

#[derive(Args)]
pub struct RunArgs {
    /// Project folder (default: current directory or a parent), or a `.http` / `.rest` / `.hurl`
    /// file to run directly.
    project: Option<PathBuf>,
    /// Environment id (file stem in environments/), e.g. `staging`. For a `.http` file: an
    /// environment from the http-client.env.json next to it.
    #[arg(short, long)]
    env: Option<String>,
    /// Variables file for file runs: `name=value` lines (Hurl `--variables-file`, .env) or a
    /// JSON object (flat, or http-client.env.json style with `--env` picking the environment).
    #[arg(long)]
    env_file: Option<PathBuf>,
    /// Set a variable (repeatable); wins over every other source: --var baseUrl=http://localhost:8080
    #[arg(long = "var", value_parser = crate::parse_kv)]
    vars: Vec<(String, String)>,
    /// Only requests whose path or name contains this text (e.g. a folder name).
    #[arg(short, long)]
    filter: Option<String>,
    /// Print response bodies.
    #[arg(short, long)]
    body: bool,
    /// Without tests, treat status >= 400 as a failure.
    #[arg(long)]
    fail_on_http_error: bool,
    /// Send even when variables are unresolved.
    #[arg(long)]
    allow_unresolved: bool,
    /// Stop at the first failure.
    #[arg(long)]
    bail: bool,
    /// Repeat the run N times.
    #[arg(short = 'n', long, default_value_t = 1)]
    iterations: usize,
    /// CSV or JSON data file: one iteration per row, columns become {{variables}}.
    #[arg(short, long)]
    data: Option<PathBuf>,
    /// Pause between requests (ms).
    #[arg(long, default_value_t = 0)]
    delay: u64,
    /// Write a JUnit XML report.
    #[arg(long)]
    junit: Option<PathBuf>,
    /// Don't read secrets from the OS keychain (use DEVALL_SECRET_<NAME> env vars and .env).
    #[arg(long)]
    no_keychain: bool,
    /// Print machine-readable JSON lines instead of a table.
    #[arg(long)]
    json: bool,
    /// Record missing response snapshots and accept changed ones (rewrites their
    /// `http/__snapshots__/` files) instead of failing the snapshot assertions.
    #[arg(long)]
    update_snapshots: bool,
}

/// CI-friendly secrets: `DEVALL_SECRET_<NAME>` env vars win, then keychain/.env.
struct CliSecrets<'a> {
    inner: SecretResolver<'a>,
}

impl SecretLookup for CliSecrets<'_> {
    fn lookup(&self, name: &str) -> Option<String> {
        std::env::var(format!("DEVALL_SECRET_{name}")).ok().or_else(|| self.inner.lookup(name))
    }
}

struct Printer {
    json: bool,
    body: bool,
    /// `http/` for project requests; empty for file runs (labels are `file.hurl#2`).
    prefix: &'static str,
}

impl RunHooks for Printer {
    fn on_item(&self, i: &RunItemResult, _sent: Option<&PreparedRequest>, resp: Option<&HttpResponseData>, secrets: &std::collections::BTreeSet<String>) {
        if self.json {
            let mut v = serde_json::to_value(i).unwrap_or_default();
            if self.body
                && let Some(r) = resp
            {
                v["body"] = serde_json::Value::String(devall_core::vars::redact_values(&String::from_utf8_lossy(&r.body), secrets));
            }
            println!("{v}");
            return;
        }
        let iter = if i.iteration > 0 { format!(" #{}", i.iteration + 1) } else { String::new() };
        if i.skipped {
            println!("↷ SKIP {}{iter}  {}{}  (pm.execution.skipRequest)", i.name, self.prefix, i.path);
        }
        // Script console output first, so it reads in execution order above the result line.
        for l in &i.scripts.logs {
            let tag = match l.level {
                devall_script::LogLevel::Warn => "⚠",
                devall_script::LogLevel::Error => "✗",
                _ => "›",
            };
            println!("    {tag} [{}] {}", l.source, l.message);
        }
        for e in &i.scripts.errors {
            println!("    ✗ {e}");
        }
        if i.skipped {
            return;
        }
        match (&i.error, i.status) {
            (Some(e), _) => println!("✗ ERR {}{iter}  {}{}\n      {e}", i.name, self.prefix, i.path),
            (None, Some(status)) => {
                let mark = if i.passed { "✓" } else { "✗" };
                println!("{mark} {status:>3} {:<7} {}{iter}  ({:.0} ms, {} B)  {}{}", i.method, i.name, i.duration_ms, i.size, self.prefix, i.path);
                for t in &i.tests {
                    let m = if t.passed { "    ✓" } else { "    ✗" };
                    let detail = match (&t.message, t.passed) {
                        // e.g. "snapshot written" with --update-snapshots
                        (Some(msg), true) => format!("  ({msg})"),
                        (_, true) => String::new(),
                        (msg, false) => format!("  — {}", msg.clone().unwrap_or_else(|| format!("actual: {}", t.actual.as_deref().unwrap_or("—")))),
                    };
                    println!("{m} {}{detail}", t.label);
                }
                for c in &i.captures {
                    match (&c.value, &c.error) {
                        (Some(_), _) => println!("    → {{{{{}}}}} captured", c.name),
                        (_, Some(e)) => println!("    → {{{{{}}}}} not captured: {e}", c.name),
                        _ => {}
                    }
                }
                if self.body
                    && let Some(r) = resp
                {
                    println!("{}\n", devall_core::vars::redact_values(&String::from_utf8_lossy(&r.body), secrets));
                }
            }
            (None, None) => {}
        }
    }
}

/// `.http`, `.rest` and `.hurl` files run directly.
fn is_request_file(path: &std::path::Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| ["http", "rest", "hurl"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn options(a: &RunArgs, data: Vec<Variables>) -> RunOptions {
    RunOptions {
        iterations: a.iterations.max(1),
        data,
        bail: a.bail,
        delay_ms: a.delay,
        fail_on_http_error: a.fail_on_http_error,
        fail_on_unresolved: !a.allow_unresolved,
        update_snapshots: a.update_snapshots,
    }
}

fn data_rows(a: &RunArgs) -> Result<Vec<Variables>> {
    match &a.data {
        Some(file) => {
            let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
            let is_json = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) || text.trim_start().starts_with('[');
            runner::parse_data_file(&text, is_json).map_err(anyhow::Error::msg)
        }
        None => Ok(Vec::new()),
    }
}

fn finish(a: &RunArgs, summary: &runner::RunSummary, suite: &str) -> Result<ExitCode> {
    if let Some(path) = &a.junit {
        std::fs::write(path, runner::to_junit(summary, suite)).with_context(|| format!("writing {}", path.display()))?;
    }
    if !a.json {
        let tests = if summary.tests_passed + summary.tests_failed > 0 {
            format!(", assertions {}/{} passed", summary.tests_passed, summary.tests_passed + summary.tests_failed)
        } else {
            String::new()
        };
        println!("\n{} passed, {} failed, {} total{tests} ({:.2}s)", summary.passed, summary.failed, summary.total, summary.duration_ms / 1000.0);
    }
    Ok(if summary.failed > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS })
}

/// Variables from `--env-file`: `name=value` lines, or JSON (flat object, or one object per
/// environment, picked by `env` or the only one).
pub(crate) fn read_env_file(path: &std::path::Path, env: Option<&str>) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}');
    let scalar = |v: &serde_json::Value| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
    if text.trim_start().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(text).with_context(|| format!("{} is not valid JSON", path.display()))?;
        let obj = v.as_object().cloned().unwrap_or_default();
        let nested = !obj.is_empty() && obj.values().all(|x| x.is_object());
        let chosen = if nested {
            let envs: Vec<&String> = obj.keys().filter(|k| *k != "$shared").collect();
            let name = match env {
                Some(e) => obj.keys().find(|k| k.eq_ignore_ascii_case(e)).cloned().with_context(|| format!("environment {e} is not in {}", path.display()))?,
                None if envs.len() == 1 => envs[0].clone(),
                None => bail!("{} has several environments ({}); pick one with --env", path.display(), envs.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")),
            };
            let mut vars: Vec<(String, String)> = obj.get("$shared").and_then(|s| s.as_object()).map(|m| m.iter().filter(|(_, v)| !v.is_object()).map(|(k, v)| (k.clone(), scalar(v))).collect()).unwrap_or_default();
            if let Some(m) = obj.get(&name).and_then(|x| x.as_object()) {
                vars.extend(m.iter().filter(|(_, v)| !v.is_object() && !v.is_array()).map(|(k, v)| (k.clone(), scalar(v))));
            }
            vars
        } else {
            obj.iter().map(|(k, v)| (k.clone(), scalar(v))).collect()
        };
        return Ok(chosen);
    }
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let v = v.trim();
            let v = if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) { &v[1..v.len() - 1] } else { v };
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect())
}

/// Runs a `.http` / `.rest` / `.hurl` file in memory.
async fn run_file(a: RunArgs, file: PathBuf) -> Result<ExitCode> {
    let plan = devall_import::parse_path(&file, &devall_import::ParseOptions::default()).with_context(|| format!("parsing {}", file.display()))?;
    if !a.json {
        for w in &plan.warnings {
            eprintln!("! {w}");
        }
    }
    let file_name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "requests".into());
    let mut requests: Vec<runner::InlineRequest> = plan
        .requests()
        .into_iter()
        .enumerate()
        .map(|(i, (_, r))| runner::InlineRequest { label: format!("{file_name}#{}", i + 1), request: r.clone() })
        .collect();
    if let Some(f) = &a.filter {
        let f = f.to_lowercase();
        requests.retain(|r| r.label.to_lowercase().contains(&f) || r.request.name.to_lowercase().contains(&f) || r.request.url.to_lowercase().contains(&f));
    }
    if requests.is_empty() {
        bail!("no requests in {}", file.display());
    }
    // File-level variables (`@name = value` in .http) are the lowest layer.
    let mut file_vars = plan.project_variables.clone();
    if let Some(s) = &plan.settings {
        file_vars.extend(s.variables.clone());
    }
    let root = file.parent().map(|p| p.to_path_buf()).filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| PathBuf::from("."));
    let project = devall_core::Project::detached(&root, &file_name, file_vars);

    let mut env = devall_core::EnvironmentFile::new(a.env.clone().unwrap_or_else(|| "cli".into()));
    if let Some(name) = &a.env {
        match plan.environments.iter().find(|e| e.name.eq_ignore_ascii_case(name)) {
            Some(e) => env.variables.extend(e.variables.clone()),
            None if a.env_file.is_none() => bail!("environment {name} not found next to {} (http-client.env.json); use --env-file", file.display()),
            None => {}
        }
    }
    if let Some(ef) = &a.env_file {
        env.variables.extend(read_env_file(ef, a.env.as_deref())?);
    }
    env.variables.extend(a.vars.iter().cloned());

    let memory = MemorySecretStore::default();
    let providers = devall_core::providers::ProviderCache::default();
    let secrets = CliSecrets { inner: SecretResolver::for_environment(&memory, project.id(), None, project.root(), Some(&env), &providers) };
    let tokens = MemoryTokenStore::default();
    let pool = ClientPool::new();
    let ctx = RunContext { project: &project, environment: Some(&env), secrets: &secrets, tokens: &tokens, browser: None, pool: &pool };
    let mut runtime = Variables::new();
    let printer = Printer { json: a.json, body: a.body, prefix: "" };
    let opts = options(&a, data_rows(&a)?);
    let summary = runner::run_requests(&ctx, &requests, &mut runtime, &opts, &printer).await;
    finish(&a, &summary, &file_name)
}

pub async fn run(a: RunArgs) -> Result<ExitCode> {
    if let Some(f) = a.project.clone().filter(|p| is_request_file(p)) {
        return run_file(a, f).await;
    }
    if a.env_file.is_some() {
        bail!("--env-file is for running .http / .hurl files; projects use environments/ (--env)");
    }
    let p = crate::find_project(a.project.clone())?;
    let env_file = a.env.as_deref().map(|id| p.read_environment(id)).transpose()?;
    let memory = MemorySecretStore::default();
    let keychain = devall_core::KeychainSecretStore::new();
    let store: &dyn SecretStore = if a.no_keychain || !devall_core::KeychainSecretStore::available() { &memory } else { &keychain };
    let providers = devall_core::providers::ProviderCache::default();
    let secrets = CliSecrets { inner: SecretResolver::for_environment(store, p.id(), a.env.as_deref(), p.root(), env_file.as_ref(), &providers) };

    let tree = p.tree()?;
    let names = names(&tree);
    let paths: Vec<String> = tree
        .request_paths()
        .into_iter()
        .filter(|path| match &a.filter {
            Some(f) => {
                let f = f.to_lowercase();
                path.to_lowercase().contains(&f) || names.get(path).is_some_and(|n| n.to_lowercase().contains(&f))
            }
            None => true,
        })
        .collect();
    if paths.is_empty() {
        bail!("no requests matched");
    }

    let opts = options(&a, data_rows(&a)?);

    // Non-interactive: client-credentials/password OAuth works; authorization code needs the app.
    let tokens = MemoryTokenStore::default();
    let pool = ClientPool::new();
    let ctx = RunContext { project: &p, environment: env_file.as_ref(), secrets: &secrets, tokens: &tokens, browser: None, pool: &pool };
    // `--var` values are runtime variables: they win over environments and folders.
    let mut runtime: Variables = a.vars.iter().cloned().collect();
    let printer = Printer { json: a.json, body: a.body, prefix: "http/" };
    let summary = runner::run(&ctx, &paths, &mut runtime, &opts, &printer).await;
    finish(&a, &summary, &p.manifest.name)
}

fn names(tree: &TreeNode) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    fn walk(n: &TreeNode, out: &mut std::collections::HashMap<String, String>) {
        match n {
            TreeNode::Folder { children, .. } => children.iter().for_each(|c| walk(c, out)),
            TreeNode::Request { name, path, .. } => {
                out.insert(path.clone(), name.clone());
            }
        }
    }
    walk(tree, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files() {
        let d = tempfile::tempdir().unwrap();
        let kv = d.path().join("vars.env");
        std::fs::write(&kv, "# comment\nbaseUrl=http://localhost:8080\nexport token=\"a=b\"\n").unwrap();
        assert_eq!(read_env_file(&kv, None).unwrap(), vec![("baseUrl".to_string(), "http://localhost:8080".to_string()), ("token".into(), "a=b".into())]);
        let js = d.path().join("http-client.env.json");
        std::fs::write(&js, r#"{"$shared":{"v":"1"},"dev":{"host":"d","n":2},"prod":{"host":"p"}}"#).unwrap();
        assert!(read_env_file(&js, None).is_err(), "ambiguous");
        assert_eq!(read_env_file(&js, Some("DEV")).unwrap(), vec![("v".to_string(), "1".to_string()), ("host".into(), "d".into()), ("n".into(), "2".into())]);
        let flat = d.path().join("flat.json");
        std::fs::write(&flat, r#"{"a":"x","b":true}"#).unwrap();
        assert_eq!(read_env_file(&flat, None).unwrap(), vec![("a".to_string(), "x".to_string()), ("b".into(), "true".into())]);
        assert!(!is_request_file(&kv));
        let h = d.path().join("api.hurl");
        std::fs::write(&h, "GET http://x\n").unwrap();
        assert!(is_request_file(&h));
    }
}
