//! `devall openapi lint | drift | sync` — spec checks for terminals and CI.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use devall_openapi::drift::SourceKind;
use devall_openapi::{Drift, MockInput, Severity, Spec, TrafficOptions};

#[derive(Subcommand)]
pub enum OpenApiCmd {
    /// Lint an OpenAPI 3.0 / 3.1 file (YAML or JSON); exits 1 on errors (or warnings with --strict).
    Lint {
        file: PathBuf,
        /// Machine-readable report.
        #[arg(long)]
        json: bool,
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
        /// Hide info-level findings.
        #[arg(short, long)]
        quiet: bool,
    },
    /// Check the project's mock stubs and/or recorded traffic (HAR) against a spec; exits 1 on errors.
    Drift {
        /// The OpenAPI file.
        #[arg(long)]
        spec: PathBuf,
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Check mocks/mappings/ (the default when no --har is given).
        #[arg(long)]
        mocks: bool,
        /// HAR file(s) of real traffic (browser dev tools, the inspector's export).
        #[arg(long)]
        har: Vec<PathBuf>,
        /// Check traffic to every host, not only the spec's servers.
        #[arg(long)]
        all_hosts: bool,
        #[arg(long)]
        json: bool,
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
    },
    /// Re-import linked specs into their request folders (every linked folder, or one).
    Sync {
        /// Folder under http/ (default: all linked folders).
        folder: Option<String>,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
}

fn mark(s: Severity) -> &'static str {
    match s {
        Severity::Error => "✗ error  ",
        Severity::Warning => "! warning",
        Severity::Info => "· info   ",
    }
}

pub async fn run(cmd: OpenApiCmd) -> Result<ExitCode> {
    match cmd {
        OpenApiCmd::Lint { file, json, strict, quiet } => {
            let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let r = devall_openapi::lint_text(&text);
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                for f in r.findings.iter().filter(|f| !(quiet && f.severity == Severity::Info)) {
                    let loc = if f.location.is_empty() { String::new() } else { format!("  [{}]", f.location) };
                    println!("{} {:<27} {}{loc}", mark(f.severity), f.rule, f.message);
                }
                println!(
                    "\n{} (OpenAPI {}): {} operations — {} errors, {} warnings, {} info",
                    r.title,
                    if r.version.is_empty() { "?" } else { &r.version },
                    r.operations,
                    r.errors,
                    r.warnings,
                    r.infos
                );
            }
            Ok(if r.errors > 0 || (strict && r.warnings > 0) { ExitCode::from(1) } else { ExitCode::SUCCESS })
        }
        OpenApiCmd::Drift { spec, project, mocks, har, all_hosts, json, strict } => {
            let s = Spec::load(&spec).with_context(|| format!("loading {}", spec.display()))?;
            let mut d = Drift::new(&s);
            let check_mocks = mocks || har.is_empty();
            if check_mocks {
                let (root, _) = crate::mock::mock_root(project)?;
                let pm = devall_mock::load_project(&root);
                for e in &pm.load_errors {
                    eprintln!("! {}: {} (not checked)", e.file, e.error);
                }
                let inputs: Vec<MockInput> = pm.mappings.into_iter().map(|m| MockInput { id: m.id, name: m.name, file: m.file, mapping: m.mapping }).collect();
                d.add_mocks(&inputs, &devall_openapi::mock_body_reader(&root));
            }
            for h in &har {
                let text = std::fs::read_to_string(h).with_context(|| format!("reading {}", h.display()))?;
                let samples = devall_openapi::har::parse_har(&text)?;
                d.add_traffic(&h.display().to_string(), &samples, &TrafficOptions { all_hosts });
            }
            let r = d.finish();
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                for f in &r.findings {
                    let what = match f.source_kind {
                        SourceKind::Mock => format!("{} ({})", f.label, f.source),
                        SourceKind::Traffic => format!("{} {}{}", f.method, f.path, if f.count > 1 { format!(" ×{}", f.count) } else { String::new() }),
                    };
                    println!("{} {what}\n            {}", mark(f.severity), f.message);
                    for det in &f.details {
                        println!("              {det}");
                    }
                }
                let uncovered: Vec<&str> = r.coverage.iter().filter(|c| c.mocks == 0 && c.traffic == 0).map(|c| c.operation.as_str()).collect();
                if !uncovered.is_empty() {
                    println!("\nNot covered by any stub{}: {}", if har.is_empty() { "" } else { " or request" }, uncovered.join(", "));
                }
                let skipped = if r.traffic_skipped > 0 { format!(" ({} to other hosts skipped; --all-hosts checks them)", r.traffic_skipped) } else { String::new() };
                println!(
                    "\n{}: {} stubs and {} requests checked{skipped} — {} errors, {} warnings, {} info",
                    r.title, r.mocks_checked, r.traffic_checked, r.errors, r.warnings, r.infos
                );
            }
            Ok(if r.errors > 0 || (strict && r.warnings > 0) { ExitCode::from(1) } else { ExitCode::SUCCESS })
        }
        OpenApiCmd::Sync { folder, project } => {
            let mut p = crate::find_project(project)?;
            let targets: Vec<String> = match folder {
                Some(f) => vec![f],
                None => devall_import::sync::links(&p)?.into_iter().map(|l| l.folder).collect(),
            };
            if targets.is_empty() {
                bail!("no folder is linked to an OpenAPI spec (link one from the app's request tree, or import a spec with “keep in sync”)");
            }
            let mut failed = false;
            for f in targets {
                match devall_import::sync::resync(&mut p, &f) {
                    Ok(r) => {
                        println!("✓ http/{} ← {}: {} updated, {} added", r.folder, r.source, r.report.requests_updated, r.report.requests_created);
                        for s in &r.stale {
                            println!("    no longer in the spec: http/{s}");
                        }
                        for w in &r.report.warnings {
                            println!("    ! {w}");
                        }
                    }
                    Err(e) => {
                        failed = true;
                        println!("✗ http/{f}: {e}");
                    }
                }
            }
            Ok(if failed { ExitCode::from(1) } else { ExitCode::SUCCESS })
        }
    }
}
