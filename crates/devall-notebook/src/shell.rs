//! Shell, PowerShell and cmd cells: one process per cell, code passed as a temporary script file
//! (no command-line quoting pitfalls), output streamed as it arrives.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Child;
use tokio::time::Instant;

use crate::ansi::StreamDecoder;
use crate::engine::RunCtx;
use crate::error::{NotebookError, Result};
use crate::interp::{Flavor, Interpreter};
use crate::process::{ProcessTree, command};

/// After interrupting, how long a process gets before it's killed.
const INTERRUPT_GRACE: Duration = Duration::from_millis(1000);
/// After the main process exits, how long background children may keep the pipes open.
const DRAIN_GRACE: Duration = Duration::from_millis(300);

/// A spawned cell process, ready to be driven by [`run`].
pub(crate) struct Spawned {
    child: Child,
    tree: ProcessTree,
    script: PathBuf,
}

fn script_ext(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Bash | Flavor::Sh => "sh",
        Flavor::Pwsh | Flavor::WindowsPowershell => "ps1",
        Flavor::Cmd => "cmd",
        Flavor::Node => "js",
        Flavor::Python => "py",
    }
}

fn script_body(flavor: Flavor, code: &str) -> String {
    let lf = code.replace("\r\n", "\n");
    match flavor {
        // CRLF breaks bash (`$'\r': command not found`).
        Flavor::Bash | Flavor::Sh => lf,
        // UTF-8 output (when a console exists), quiet echo, CRLF line endings.
        Flavor::Cmd => format!("@echo off\r\nchcp 65001 >nul 2>&1\r\n{}\r\n", lf.replace('\n', "\r\n")),
        _ => lf,
    }
}

/// Runs the script with UTF-8 output, through a script block so the execution policy for `.ps1`
/// files doesn't apply; exit code = `exit N`, else the last native exit code, else 1 when the
/// script threw.
fn powershell_bootstrap(script: &Path) -> String {
    let p = script.to_string_lossy().replace('\'', "''");
    format!(
        "$ErrorActionPreference = 'Continue'; $ProgressPreference = 'SilentlyContinue'\n\
         try {{ [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false) }} catch {{}}\n\
         $OutputEncoding = [Text.UTF8Encoding]::new($false)\n\
         $global:LASTEXITCODE = 0\n\
         $__devallOk = $true\n\
         try {{ & ([scriptblock]::Create([IO.File]::ReadAllText('{p}', [Text.Encoding]::UTF8))) }} catch {{ $__devallOk = $false; [Console]::Error.WriteLine(($_ | Out-String)) }}\n\
         if (-not $__devallOk) {{ exit 1 }} elseif ($LASTEXITCODE) {{ exit $LASTEXITCODE }} else {{ exit 0 }}\n"
    )
}

/// Writes the script and starts the interpreter.
pub(crate) fn spawn(
    interp: &Interpreter,
    code: &str,
    cwd: &Path,
    env: &[(String, String)],
    env_path: Option<&OsString>,
    scratch: &Path,
) -> Result<Spawned> {
    std::fs::create_dir_all(scratch)?;
    let script = scratch.join(format!("cell-{}.{}", ulid::Ulid::generate().to_string().to_lowercase(), script_ext(interp.flavor)));
    std::fs::write(&script, script_body(interp.flavor, code))?;

    let mut cmd = command(&interp.program);
    cmd.args(&interp.args);
    match interp.flavor {
        Flavor::Bash => {
            if cfg!(windows) {
                // Git Bash: bin\bash.exe already sets up PATH; a login shell costs ~1 s per cell.
                cmd.env("CHERE_INVOKING", "1");
                cmd.arg(script.to_string_lossy().replace('\\', "/"));
            } else {
                cmd.arg("-l").arg(&script);
            }
        }
        Flavor::Sh | Flavor::Node | Flavor::Python => {
            cmd.arg(&script);
        }
        Flavor::Pwsh | Flavor::WindowsPowershell => {
            cmd.args(["-NoProfile", "-NonInteractive", "-Command"]).arg(powershell_bootstrap(&script));
        }
        Flavor::Cmd => {
            #[cfg(windows)]
            cmd.raw_arg(format!("/d /s /c \"\"{}\"\"", script.display()));
            #[cfg(not(windows))]
            return Err(NotebookError::Unsupported("cmd cells only run on Windows".into()));
        }
    }
    if let Some(p) = env_path {
        cmd.env("PATH", p);
    }
    cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|source| {
        let _ = std::fs::remove_file(&script);
        NotebookError::Spawn { program: interp.program.clone(), source }
    })?;
    let tree = ProcessTree::attach(&child);
    Ok(Spawned { child, tree, script })
}

fn pump<R: AsyncRead + Unpin + Send + 'static>(mut r: R, ctx: RunCtx, stderr: bool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut dec = StreamDecoder::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = dec.push(&buf[..n]);
                    ctx.output(stderr, &text);
                }
            }
        }
        let rest = dec.flush();
        ctx.output(stderr, &rest);
    })
}

/// Drives a spawned cell to completion: (exit code, interrupted, timed out).
pub(crate) async fn run(mut sp: Spawned, ctx: &RunCtx) -> (Option<i32>, bool, bool) {
    let out = sp.child.stdout.take().map(|o| pump(o, ctx.clone(), false));
    let err = sp.child.stderr.take().map(|e| pump(e, ctx.clone(), true));
    let mut stop = ctx.stop_signal();
    let deadline = ctx.timeout.map(|t| Instant::now() + t);
    let (mut interrupted, mut timed_out) = (false, false);
    let mut kill_at: Option<Instant> = None;
    let status = loop {
        tokio::select! {
            s = sp.child.wait() => break s.ok(),
            _ = crate::engine::stopped(&mut stop), if !interrupted => {
                interrupted = true;
                sp.tree.interrupt();
                kill_at = Some(Instant::now() + INTERRUPT_GRACE);
            }
            _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() && !interrupted => {
                interrupted = true;
                timed_out = true;
                sp.tree.interrupt();
                kill_at = Some(Instant::now() + INTERRUPT_GRACE);
            }
            _ = tokio::time::sleep_until(kill_at.unwrap_or_else(Instant::now)), if kill_at.is_some() => {
                kill_at = None;
                sp.tree.kill();
            }
        }
    };
    // Background children may hold the pipes open; give them a moment, then end the tree (a cell
    // owns everything it started).
    let readers: Vec<_> = out.into_iter().chain(err).collect();
    let all_done = |rs: &[tokio::task::JoinHandle<()>]| rs.iter().all(|r| r.is_finished());
    wait_until(|| all_done(&readers), DRAIN_GRACE).await;
    sp.tree.kill();
    wait_until(|| all_done(&readers), Duration::from_millis(500)).await;
    for r in readers {
        if r.is_finished() {
            let _ = r.await;
        } else {
            r.abort();
        }
    }
    let _ = std::fs::remove_file(&sp.script);
    (status.and_then(|s| s.code()), interrupted, timed_out)
}

async fn wait_until(mut done: impl FnMut() -> bool, max: Duration) {
    let end = Instant::now() + max;
    while !done() && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_bodies() {
        assert_eq!(script_body(Flavor::Bash, "echo a\r\necho b"), "echo a\necho b");
        assert!(script_body(Flavor::Cmd, "echo a\necho b").ends_with("echo a\r\necho b\r\n"));
        let b = powershell_bootstrap(Path::new("C:\\it's\\cell.ps1"));
        assert!(b.contains("'C:\\it''s\\cell.ps1'"));
        assert!(!b.contains('"'), "the bootstrap is passed as one argument; no double quotes");
    }
}
