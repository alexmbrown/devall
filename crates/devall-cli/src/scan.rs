//! `devall scan`: find credentials in project files (and staged changes, as a git pre-commit hook).

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use clap::Args;
use devall_core::scan::{self, FileFindings};

#[derive(Args)]
pub struct ScanArgs {
    /// Folder to scan (default: the current project or directory).
    path: Option<PathBuf>,
    /// Scan only files staged for commit (what a pre-commit hook needs).
    #[arg(long)]
    staged: bool,
    /// Install a git pre-commit hook that runs `devall scan --staged`.
    #[arg(long)]
    install_hook: bool,
    /// Machine-readable output.
    #[arg(long)]
    json: bool,
}

fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).output().context("git is not installed or not on PATH")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn run(a: ScanArgs) -> Result<ExitCode> {
    if a.install_hook {
        let hooks = PathBuf::from(git(&["rev-parse", "--git-path", "hooks"])?.trim());
        std::fs::create_dir_all(&hooks)?;
        let hook = hooks.join("pre-commit");
        if hook.exists() && !std::fs::read_to_string(&hook).unwrap_or_default().contains("devall scan") {
            bail!("{} already exists; add `devall scan --staged` to it yourself", hook.display());
        }
        std::fs::write(&hook, scan::PRE_COMMIT_HOOK)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
        }
        println!("Installed {}", hook.display());
        return Ok(ExitCode::SUCCESS);
    }

    let results: Vec<FileFindings> = if a.staged {
        let files = git(&["diff", "--cached", "--name-only", "--diff-filter=ACM"])?;
        files
            .lines()
            .filter(|f| {
                let l = f.to_ascii_lowercase();
                [".yaml", ".yml", ".json", ".md", ".ipynb", ".http", ".env"].iter().any(|e| l.ends_with(e)) && !l.contains(".devall/")
            })
            .filter_map(|f| {
                let content = git(&["show", &format!(":{f}")]).ok()?;
                let findings = scan::scan_text(&content);
                (!findings.is_empty()).then(|| FileFindings { path: f.to_string(), findings })
            })
            .collect()
    } else {
        let root = match a.path {
            Some(p) => p,
            None => crate::find_project(None).map(|p| p.root().to_path_buf()).or_else(|_| std::env::current_dir())?,
        };
        scan::scan_project(&root)
    };

    if a.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("✓ no credentials found");
    } else {
        for f in &results {
            for x in &f.findings {
                println!("✗ {}:{}:{}  {}  ({})", f.path, x.line, x.column, x.description, x.excerpt);
            }
        }
        let n: usize = results.iter().map(|f| f.findings.len()).sum();
        println!("\n{n} possible credential(s). Move values into secrets ({{{{secret.NAME}}}}) — they stay in your OS keychain.");
    }
    Ok(if results.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
