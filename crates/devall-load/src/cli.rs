//! Helpers for a `devall load run <file> [--env x] [--yes]` command: one call that plans, enforces
//! the confirmation guard rail, runs, writes the summary and maps the result to an exit code.

use std::path::{Path, PathBuf};

use devall_core::{Project, SecretStore};

use crate::distributed::{DistributedOptions, run_distributed};
use crate::engine::{ProgressFn, RunControl, RunOptions, run};
use crate::error::{LoadError, Result};
use crate::plan::{Preflight, build_plan, effective_environment};
use crate::store::{read_test, summary_json, write_run};
use crate::summary::LoadSummary;

/// Everything passed and the run finished.
pub const EXIT_OK: i32 = 0;
/// Planning or I/O failed (bad file, missing request, …).
pub const EXIT_ERROR: i32 = 1;
/// The run needs `--yes` (non-local target at a high rate, or a protected environment).
pub const EXIT_CONFIRMATION_REQUIRED: i32 = 2;
/// At least one threshold failed (same code as k6).
pub const EXIT_THRESHOLDS_FAILED: i32 = 99;
/// The run was stopped early (Ctrl-C, port exhaustion) — same code as k6's external abort.
pub const EXIT_ABORTED: i32 = 105;

/// Exit code for a finished run.
pub fn exit_code(summary: &LoadSummary) -> i32 {
    if summary.aborted {
        EXIT_ABORTED
    } else if !summary.thresholds_passed {
        EXIT_THRESHOLDS_FAILED
    } else {
        EXIT_OK
    }
}

/// Exit code for an error returned by [`run_test_file`].
pub fn error_exit_code(e: &LoadError) -> i32 {
    match e {
        LoadError::ConfirmationRequired(_) => EXIT_CONFIRMATION_REQUIRED,
        _ => EXIT_ERROR,
    }
}

#[derive(Default)]
pub struct CliRunOptions {
    /// `--env`: overrides the test's `environment`.
    pub environment: Option<String>,
    /// `--yes`: accept the confirmation guard rail.
    pub confirmed: bool,
    /// Save the summary in the project data folder on this device (default in the CLI: yes).
    pub write_summary: bool,
    /// Also write the summary (secrets masked) to this file — CI artifacts (`--out`).
    pub summary_out: Option<PathBuf>,
    pub progress: Option<ProgressFn>,
    /// Stop handle (wire Ctrl-C to [`RunControl::stop`]).
    pub control: RunControl,
    /// Base URL of the project's running mock server: the run targets it (`--against-mock`, or
    /// tests with `target: mock`) and is saved as a mock baseline.
    pub mock_url: Option<String>,
    /// Split the run across `devall load agent` workers (`--agents …`).
    pub distributed: Option<DistributedOptions>,
}

/// `true` when the test at `load/<test_path>` has `target: mock` (start the mock first).
pub fn wants_mock(root: &Path, test_path: &str) -> Result<bool> {
    Ok(read_test(root, test_path)?.target == crate::model::LoadTarget::Mock)
}

pub struct CliOutcome {
    pub preflight: Preflight,
    pub summary: LoadSummary,
    /// Run id of the saved summary (see [`crate::store::run_path`]), when written.
    pub file: Option<String>,
    pub exit_code: i32,
}

/// Runs `load/<test_path>` in the project at `root`.
pub async fn run_test_file(root: &Path, test_path: &str, secrets: &dyn SecretStore, opts: CliRunOptions) -> Result<CliOutcome> {
    let project = Project::open(root)?;
    let test = read_test(project.root(), test_path)?;
    let env = effective_environment(&test, opts.environment.as_deref(), None);
    let mut plan = build_plan(&project, test_path, &test, env.as_deref(), secrets)?;
    if test.target == crate::model::LoadTarget::Mock && opts.mock_url.is_none() {
        return Err(LoadError::Invalid("this test targets the mock server (target: mock); start it first or run with --against-mock".into()));
    }
    if let Some(url) = &opts.mock_url {
        if opts.distributed.is_some() {
            return Err(LoadError::Invalid("mock baselines run against this machine's mock server; drop --agents (or --against-mock)".into()));
        }
        plan.target_mock(url)?;
    }
    let preflight = plan.preflight();
    if let Some(c) = &preflight.requires_confirmation
        && !opts.confirmed
    {
        return Err(LoadError::ConfirmationRequired(format!("{} Re-run with --yes to proceed.", c.reason)));
    }
    let secret_values = plan.secret_values.clone();
    let summary = match opts.distributed {
        Some(d) => run_distributed(plan, d, opts.progress, opts.control).await?,
        None => run(plan, RunOptions::default(), opts.progress, opts.control).await?,
    };
    let file = if opts.write_summary { Some(write_run(project.root(), &summary, &secret_values)?) } else { None };
    if let Some(out) = &opts.summary_out {
        devall_core::fsutil::atomic_write(out, summary_json(&summary, &secret_values).as_bytes())?;
    }
    let exit_code = exit_code(&summary);
    Ok(CliOutcome { preflight, summary, file, exit_code })
}

/// Plain-text report for terminals.
pub fn format_summary(s: &LoadSummary) -> String {
    let mut out = String::new();
    let status = if s.aborted {
        format!("ABORTED ({})", s.abort_reason.as_deref().unwrap_or("stopped"))
    } else if s.thresholds_passed {
        "PASSED".into()
    } else {
        "FAILED".into()
    };
    out.push_str(&format!("{} — {status}{}\n", s.test, if s.mock_baseline.is_some() { " (mock baseline)" } else { "" }));
    out.push_str(&format!(
        "  {} · {:.1}s · {}{}\n",
        s.executor.kind(),
        s.duration_ms as f64 / 1000.0,
        if s.open_model { "latency from intended start (CO-corrected)" } else { "latency from send (closed model)" },
        s.environment.as_deref().map(|e| format!(" · env {e}")).unwrap_or_default()
    ));
    let t = &s.totals;
    out.push_str(&format!(
        "  requests {}  ({:.1}/s)   failed {} ({:.2}%)   iterations {}   dropped {}   max VUs {}\n",
        t.requests,
        t.rps,
        t.failed,
        t.error_rate * 100.0,
        t.iterations,
        t.dropped_iterations,
        t.max_vus
    ));
    let l = &s.latency;
    out.push_str(&format!(
        "  latency ms  p50 {}  p90 {}  p95 {}  p99 {}  p99.9 {}  max {}  mean {}\n",
        l.p50, l.p90, l.p95, l.p99, l.p999, l.max, l.mean
    ));
    if let Some(sv) = &s.service_time {
        out.push_str(&format!("  service ms  p50 {}  p95 {}  p99 {}  (from actual send)\n", sv.p50, sv.p95, sv.p99));
    }
    if !s.status_codes.is_empty() {
        let codes: Vec<String> = s.status_codes.iter().map(|(k, v)| format!("{k}×{v}")).collect();
        out.push_str(&format!("  status      {}\n", codes.join("  ")));
    }
    let e = &s.error_kinds;
    if e.total() > 0 {
        out.push_str(&format!("  errors      connect {}  timeout {}  other {}\n", e.connect, e.timeout, e.other));
    }
    if let Some(agents) = &s.agents {
        out.push_str(&format!("  agents      {} (percentiles and thresholds from the merged HDR histograms)\n", agents.len()));
        for a in agents {
            let state = match (&a.error, a.ok) {
                (Some(e), _) => format!("FAILED: {e}"),
                (None, true) => format!("{} requests, {} failed, p95 {} ms", a.requests, a.failed, a.p95),
                (None, false) => "no results".to_string(),
            };
            out.push_str(&format!("    {} (share {:.0}%, {} {} cpus) — {state}\n", a.address, a.share * 100.0, a.machine.os, a.machine.cpus));
        }
    }
    for th in &s.thresholds {
        out.push_str(&format!("  {} {}\n", if th.ok { "✓" } else { "✗" }, th.message));
    }
    for w in &s.warnings {
        out.push_str(&format!("  ! {w}\n"));
    }
    out
}
