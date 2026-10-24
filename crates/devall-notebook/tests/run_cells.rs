//! Runs real cells with whatever interpreters this machine has; languages whose interpreter is
//! missing are skipped (with a note on stderr).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use devall_notebook::{CellEvent, CellResult, Engine, Flavor, Language, RunOptions};

struct Env {
    engine: Engine,
    dir: tempfile::TempDir,
}

fn setup() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(dir.path().join("scratch"));
    Env { engine, dir }
}

impl Env {
    fn opts(&self, key: &str) -> RunOptions {
        RunOptions { cwd: self.dir.path().to_path_buf(), kernel_key: key.to_string(), ..Default::default() }
    }

    async fn have(&self, language: Language) -> Option<Flavor> {
        match self.engine.interpreters().get(language).await {
            Ok(i) => Some(i.flavor),
            Err(e) => {
                eprintln!("skipping {language}: {e}");
                None
            }
        }
    }

    async fn run(&self, language: Language, code: &str, opts: RunOptions) -> (Vec<CellEvent>, CellResult) {
        let h = self.engine.start(code, language, opts).await.unwrap_or_else(|e| panic!("start {language}: {e}"));
        let mut events = Vec::new();
        let r = h.finish(|e| events.push(e.clone())).await;
        (events, r)
    }
}

fn stdout(events: &[CellEvent]) -> String {
    events.iter().filter_map(|e| if let CellEvent::Stdout { plain, .. } = e { Some(plain.as_str()) } else { None }).collect()
}

fn stderr(events: &[CellEvent]) -> String {
    events.iter().filter_map(|e| if let CellEvent::Stderr { plain, .. } = e { Some(plain.as_str()) } else { None }).collect()
}

fn result_text(events: &[CellEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        CellEvent::Display { data, result: true, .. } => data.get("text/plain").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    })
}

fn error_name(events: &[CellEvent]) -> Option<String> {
    events.iter().find_map(|e| if let CellEvent::Error { ename, .. } = e { Some(ename.clone()) } else { None })
}

fn is_posix_shell(f: Flavor) -> bool {
    matches!(f, Flavor::Bash | Flavor::Sh)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shell_output_exit_codes_and_env() {
    let t = setup();
    let Some(flavor) = t.have(Language::Shell).await else { return };
    let code = if is_posix_shell(flavor) {
        "echo \"hello $DEVALL_greeting\"\necho oops 1>&2\nexit 3"
    } else {
        "Write-Output \"hello $env:DEVALL_greeting\"\n[Console]::Error.WriteLine('oops')\nexit 3"
    };
    let mut o = t.opts("nb");
    o.env.push(("DEVALL_greeting".into(), "world ✓".into()));
    let (ev, r) = t.run(Language::Shell, code, o).await;
    assert_eq!(stdout(&ev), "hello world ✓\n", "{ev:?}");
    assert!(stderr(&ev).contains("oops"), "{ev:?}");
    assert_eq!(r.exit_code, Some(3));
    assert!(!r.ok);
    assert!(matches!(ev.first(), Some(CellEvent::Started { language: Language::Shell, .. })));
    assert!(ev.last().unwrap().is_exit());
    assert_eq!(r.outputs[0]["output_type"], "stream");

    let ok_code = if is_posix_shell(flavor) { "pwd; true" } else { "(Get-Location).Path" };
    let (ev, r) = t.run(Language::Shell, ok_code, t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    let dir_name = t.dir.path().file_name().unwrap().to_string_lossy().to_string();
    assert!(stdout(&ev).contains(&dir_name), "runs in the requested cwd: {}", stdout(&ev));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn powershell_utf8_and_exit_code() {
    let t = setup();
    if t.have(Language::Powershell).await.is_none() {
        return;
    }
    let (ev, r) = t.run(Language::Powershell, "Write-Output 'héllo ✓'\n[Console]::Error.WriteLine('err line')\nexit 4", t.opts("nb")).await;
    assert_eq!(stdout(&ev).trim(), "héllo ✓", "{ev:?}");
    assert!(stderr(&ev).contains("err line"));
    assert_eq!(r.exit_code, Some(4));
    let (ev, r) = t.run(Language::Powershell, "throw 'boom'", t.opts("nb")).await;
    assert_eq!(r.exit_code, Some(1));
    assert!(stderr(&ev).contains("boom"), "{ev:?}");
    let (_, r) = t.run(Language::Powershell, "$x = 1\nif ($x -eq 1) {\n  'one'\n}", t.opts("nb")).await;
    assert!(r.ok);
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cmd_cells_on_windows() {
    let t = setup();
    if t.have(Language::Cmd).await.is_none() {
        return;
    }
    let (ev, r) = t.run(Language::Cmd, "echo cmd works\r\nexit /b 2", t.opts("nb")).await;
    assert_eq!(stdout(&ev).trim(), "cmd works", "{ev:?}");
    assert_eq!(r.exit_code, Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn javascript_kernel_keeps_state() {
    let t = setup();
    if t.have(Language::Javascript).await.is_none() {
        return;
    }
    let (ev, r) = t.run(Language::Javascript, "const x = 40;\nlet y = await Promise.resolve(2);\nconsole.log('ready', x)", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(stdout(&ev), "ready 40\n");
    assert_eq!(result_text(&ev), None, "console.log returns undefined");

    let (ev, r) = t.run(Language::Javascript, "x + y", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(result_text(&ev).as_deref(), Some("42"));
    assert_eq!(r.outputs[0]["output_type"], "execute_result");

    // Re-running a cell that declares `const` works (REPL mode), require and static imports work.
    let (ev, r) = t.run(Language::Javascript, "const x = 1;\nimport os from 'node:os';\nconst { platform } = require('node:process');\n[x, typeof os.cpus, typeof platform]", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(result_text(&ev).as_deref(), Some("[ 1, 'function', 'string' ]"));

    let (ev, r) = t.run(Language::Javascript, "process.stdout.write('raw\\n'); console.error('warn'); throw new TypeError('boom')", t.opts("nb")).await;
    assert!(!r.ok);
    assert_eq!(r.exit_code, Some(1));
    assert_eq!(error_name(&ev).as_deref(), Some("TypeError"));
    assert!(stdout(&ev).contains("raw"), "{ev:?}");
    assert!(stderr(&ev).contains("warn") || stdout(&ev).contains("warn"), "{ev:?}");

    // Child processes that inherit stdio write straight into the cell.
    let (ev, r) = t.run(Language::Javascript, "require('node:child_process').spawnSync(process.execPath, ['-e', 'console.log(\"from child\")'], { stdio: 'inherit' }).status", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert!(stdout(&ev).contains("from child"), "{ev:?}");

    // Another notebook has its own kernel.
    let (ev, _) = t.run(Language::Javascript, "typeof y", t.opts("other")).await;
    assert_eq!(result_text(&ev).as_deref(), Some("'undefined'"));

    // Environment variables arrive per cell.
    let mut o = t.opts("nb");
    o.env.push(("DEVALL_baseUrl".into(), "http://x".into()));
    let (ev, _) = t.run(Language::Javascript, "process.env.DEVALL_baseUrl", o).await;
    assert_eq!(result_text(&ev).as_deref(), Some("'http://x'"));
    t.engine.shutdown_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn python_kernel_keeps_state() {
    let t = setup();
    if t.have(Language::Python).await.is_none() {
        return;
    }
    let (ev, r) = t.run(Language::Python, "x = 40\nprint('ready', x)", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(stdout(&ev), "ready 40\n");
    let (ev, r) = t.run(Language::Python, "x + 2", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(result_text(&ev).as_deref(), Some("42"));
    let (ev, _) = t.run(Language::Python, "x + 2;", t.opts("nb")).await;
    assert_eq!(result_text(&ev), None, "a trailing ; hides the value");

    let (ev, r) = t.run(Language::Python, "import asyncio\nawait asyncio.sleep(0.01)\n{'a': [1, 2]}", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    assert_eq!(result_text(&ev).as_deref(), Some("{'a': [1, 2]}"));

    let (ev, r) = t.run(Language::Python, "class H:\n    def _repr_html_(self):\n        return '<b>hi</b>'\nH()", t.opts("nb")).await;
    assert!(r.ok);
    let html = ev.iter().find_map(|e| if let CellEvent::Display { data, .. } = e { data.get("text/html").cloned() } else { None });
    assert_eq!(html.and_then(|v| v.as_str().map(String::from)).as_deref(), Some("<b>hi</b>"));

    let (ev, r) = t.run(Language::Python, "import sys\nprint('before')\nprint('to err', file=sys.stderr)\n1/0", t.opts("nb")).await;
    assert!(!r.ok);
    assert_eq!(error_name(&ev).as_deref(), Some("ZeroDivisionError"));
    assert_eq!(stdout(&ev), "before\n");
    assert_eq!(stderr(&ev), "to err\n");
    let tb = ev.iter().find_map(|e| if let CellEvent::Error { traceback, .. } = e { Some(traceback.join("\n")) } else { None }).unwrap();
    assert!(tb.contains("<cell-") && !tb.contains("devall_python_kernel"), "{tb}");
    let err = r.outputs.iter().find(|o| o["output_type"] == "error").unwrap();
    assert_eq!(err["ename"], "ZeroDivisionError");

    let (ev, r) = t.run(Language::Python, "def broken(:\n  pass", t.opts("nb")).await;
    assert!(!r.ok);
    assert_eq!(error_name(&ev).as_deref(), Some("SyntaxError"));

    // Subprocess output (raw fd 1) still lands in the cell.
    let (ev, r) = t.run(Language::Python, "import subprocess, sys\nsubprocess.run([sys.executable, '-c', 'print(\"from child\")'])\nprint('after')", t.opts("nb")).await;
    assert!(r.ok, "{ev:?}");
    let out = stdout(&ev);
    assert!(out.contains("from child") && out.contains("after"), "{out}");

    // Restart clears state.
    assert_eq!(t.engine.restart("nb").await, 1);
    let (ev, _) = t.run(Language::Python, "x", t.opts("nb")).await;
    assert_eq!(error_name(&ev).as_deref(), Some("NameError"));
    t.engine.shutdown_all().await;
}

/// Starts a long-running cell, interrupts it after `after`, and returns how long the interrupt
/// took plus the result.
async fn interrupt_after(t: &Env, language: Language, code: &str, key: &str, after: Duration) -> (Duration, Vec<CellEvent>, CellResult) {
    let mut o = t.opts(key);
    o.run_id = Some(format!("run-{key}"));
    let h = t.engine.start(code, language, o).await.unwrap();
    let id = h.run_id.clone();
    let collect = tokio::spawn(async move {
        let mut events = Vec::new();
        let r = h.finish(|e| events.push(e.clone())).await;
        (events, r, Instant::now())
    });
    tokio::time::sleep(after).await;
    let asked = Instant::now();
    assert!(t.engine.interrupt(&id));
    let (events, r, done) = tokio::time::timeout(Duration::from_secs(10), collect).await.expect("cell stopped").unwrap();
    (done.duration_since(asked), events, r)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupts_long_running_cells_within_two_seconds() {
    let t = setup();
    if let Some(flavor) = t.have(Language::Shell).await {
        let code = if is_posix_shell(flavor) {
            "echo started\nwhile true; do sleep 0.1; done"
        } else {
            "Write-Output started\nwhile ($true) { Start-Sleep -Milliseconds 100 }"
        };
        let (took, ev, r) = interrupt_after(&t, Language::Shell, code, "sh", Duration::from_millis(800)).await;
        assert!(took < Duration::from_secs(2), "shell interrupt took {took:?}");
        assert!(r.interrupted && !r.ok, "{r:?}");
        assert!(stdout(&ev).contains("started"));
        assert!(t.engine.running().is_empty());
    }
    if t.have(Language::Python).await.is_some() {
        t.run(Language::Python, "kept = 'yes'", t.opts("py")).await;
        let (took, ev, r) = interrupt_after(&t, Language::Python, "import time\nwhile True:\n    time.sleep(0.1)", "py", Duration::from_millis(500)).await;
        assert!(took < Duration::from_secs(2), "python interrupt took {took:?}");
        assert!(r.interrupted);
        assert_eq!(error_name(&ev).as_deref(), Some("KeyboardInterrupt"), "{ev:?}");
        let (ev, _) = t.run(Language::Python, "kept", t.opts("py")).await;
        assert_eq!(result_text(&ev).as_deref(), Some("'yes'"), "soft interrupt keeps variables");
        // A long sleep (one blocking call) is interrupted too.
        let (took, _, r) = interrupt_after(&t, Language::Python, "import time\ntime.sleep(60)", "py", Duration::from_millis(300)).await;
        assert!(took < Duration::from_secs(2) && r.interrupted, "{took:?}");
    }
    if t.have(Language::Javascript).await.is_some() {
        t.run(Language::Javascript, "const kept = 'yes'", t.opts("js")).await;
        let (took, _, r) = interrupt_after(&t, Language::Javascript, "let n = 0;\nwhile (true) { n++ }", "js", Duration::from_millis(500)).await;
        assert!(took < Duration::from_secs(2), "js sync interrupt took {took:?}");
        assert!(r.interrupted);
        let (ev, _) = t.run(Language::Javascript, "kept", t.opts("js")).await;
        assert_eq!(result_text(&ev).as_deref(), Some("'yes'"), "busy-loop interrupt keeps variables: {ev:?}");
        let (took, _, r) = interrupt_after(&t, Language::Javascript, "for (;;) { await new Promise((r) => setTimeout(r, 100)); }", "js", Duration::from_millis(500)).await;
        assert!(took < Duration::from_secs(2) && r.interrupted, "js async interrupt took {took:?}");
    }
    t.engine.shutdown_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeouts_redaction_and_missing_interpreters() {
    let t = setup();
    if let Some(flavor) = t.have(Language::Shell).await {
        let mut o = t.opts("nb");
        o.timeout = Some(Duration::from_millis(400));
        let code = if is_posix_shell(flavor) { "sleep 20" } else { "Start-Sleep -Seconds 20" };
        let started = Instant::now();
        let (_, r) = t.run(Language::Shell, code, o).await;
        assert!(r.timed_out && r.interrupted, "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));

        let mut o = t.opts("nb");
        o.redact = BTreeSet::from(["hunter22-secret".to_string()]);
        let code = if is_posix_shell(flavor) { "echo token=hunter22-secret" } else { "Write-Output 'token=hunter22-secret'" };
        let (ev, r) = t.run(Language::Shell, code, o).await;
        assert!(!stdout(&ev).contains("hunter22"), "{ev:?}");
        assert!(!r.outputs[0].to_string().contains("hunter22"));
    }
    t.engine.interpreters().set_override(Language::Python, Some(PathBuf::from("/no/such/python")));
    let err = t.engine.start("1", Language::Python, t.opts("nb")).await.err().expect("missing interpreter").to_string();
    assert!(err.contains("Settings"), "{err}");
    if !cfg!(windows) {
        let err = t.engine.start("echo", Language::Cmd, t.opts("nb")).await.err().unwrap().to_string();
        assert!(err.contains("only run on Windows"), "{err}");
    }
    assert!(!t.engine.interrupt("unknown-run"));
}
