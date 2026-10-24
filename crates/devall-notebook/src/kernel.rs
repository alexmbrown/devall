//! Persistent Node.js and Python kernels: one long-lived process per notebook and language,
//! driven by a small driver script (see `drivers/`) over stdin/stdout JSON lines.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::ansi::StreamDecoder;
use crate::engine::{RunCtx, stopped};
use crate::error::{NotebookError, Result};
use crate::interp::{Flavor, Interpreter};
use crate::process::{ProcessTree, command};

/// Protocol lines on a kernel's stdout start with this (anything else is raw cell output).
pub(crate) const MARK: &str = "\u{1e}DEVALL";
pub(crate) const NODE_DRIVER: &str = include_str!("drivers/node_driver.js");
pub(crate) const PYTHON_DRIVER: &str = include_str!("drivers/python_driver.py");

const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// After a soft interrupt (KeyboardInterrupt / terminateExecution), how long before the kernel is
/// killed.
const INTERRUPT_GRACE: Duration = Duration::from_millis(1500);
const LOG_CAP: usize = 16 * 1024;

#[derive(Debug)]
enum KMsg {
    Out { stderr: bool, text: String },
    Display { data: Map<String, Value>, result: bool },
    Error { ename: String, evalue: String, traceback: Vec<String> },
    Done { ok: bool, interrupted: bool },
    Exited,
}

#[derive(Default)]
struct Shared {
    /// The executing cell: (exec id, sink).
    current: Option<(u64, mpsc::UnboundedSender<KMsg>)>,
    /// stderr while no cell runs (startup errors).
    log: String,
    ready: Option<oneshot::Sender<Option<String>>>,
}

impl Shared {
    fn route(&mut self, id: Option<u64>, msg: KMsg) {
        if let Some((cur, tx)) = &self.current
            && id.is_none_or(|i| i == *cur)
        {
            let _ = tx.send(msg);
        } else if let KMsg::Out { text, .. } = msg
            && self.log.len() < LOG_CAP
        {
            self.log.push_str(&text);
        }
    }
}

pub(crate) struct Kernel {
    pub interpreter: Interpreter,
    pub version: Option<String>,
    stdin: tokio::sync::Mutex<ChildStdin>,
    tree: ProcessTree,
    _child: Child,
    shared: Arc<Mutex<Shared>>,
    alive: Arc<AtomicBool>,
    /// Serializes cells: a kernel runs one at a time.
    pub exec_lock: tokio::sync::Mutex<()>,
    /// Secret values that ever entered this kernel; redacted from all of its later output.
    redact: Mutex<BTreeSet<String>>,
}

impl Drop for Kernel {
    fn drop(&mut self) {
        self.tree.kill();
    }
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn handle_line(shared: &Mutex<Shared>, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let mut s = shared.lock().unwrap();
    let Some(pos) = text.find(MARK) else {
        s.route(None, KMsg::Out { stderr: false, text: text.into_owned() });
        return;
    };
    if pos > 0 {
        s.route(None, KMsg::Out { stderr: false, text: text[..pos].to_string() });
    }
    let Ok(v) = serde_json::from_str::<Value>(text[pos + MARK.len()..].trim_end()) else { return };
    let id = v.get("id").and_then(Value::as_u64);
    let msg = match v.get("t").and_then(Value::as_str).unwrap_or("") {
        "ready" => {
            if let Some(tx) = s.ready.take() {
                let _ = tx.send(Some(str_field(&v, "version")).filter(|v| !v.is_empty()));
            }
            return;
        }
        "stream" => KMsg::Out { stderr: v.get("name").and_then(Value::as_str) == Some("stderr"), text: str_field(&v, "text") },
        "display" => KMsg::Display {
            data: v.get("data").and_then(Value::as_object).cloned().unwrap_or_default(),
            result: v.get("result").and_then(Value::as_bool).unwrap_or(false),
        },
        "error" => KMsg::Error {
            ename: str_field(&v, "ename"),
            evalue: str_field(&v, "evalue"),
            traceback: v.get("traceback").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        },
        "done" => KMsg::Done {
            ok: v.get("ok").and_then(Value::as_bool).unwrap_or(false),
            interrupted: v.get("interrupted").and_then(Value::as_bool).unwrap_or(false),
        },
        _ => return,
    };
    s.route(id, msg);
}

fn read_stdout(out: ChildStdout, shared: Arc<Mutex<Shared>>, alive: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut r = BufReader::new(out);
        let mut line = Vec::with_capacity(4096);
        loop {
            line.clear();
            match r.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => handle_line(&shared, &line),
            }
        }
        alive.store(false, Ordering::SeqCst);
        let mut s = shared.lock().unwrap();
        if let Some(tx) = s.ready.take() {
            let _ = tx.send(None);
        }
        if let Some((_, tx)) = &s.current {
            let _ = tx.send(KMsg::Exited);
        }
    });
}

fn read_stderr<R: tokio::io::AsyncRead + Unpin + Send + 'static>(mut err: R, shared: Arc<Mutex<Shared>>) {
    tokio::spawn(async move {
        let mut dec = StreamDecoder::new();
        let mut buf = vec![0u8; 8192];
        loop {
            match err.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = dec.push(&buf[..n]);
                    if !text.is_empty() {
                        shared.lock().unwrap().route(None, KMsg::Out { stderr: true, text });
                    }
                }
            }
        }
    });
}

impl Kernel {
    /// Starts a kernel and waits until its driver reports ready.
    pub async fn start(interp: &Interpreter, driver: &Path, cwd: &Path, env_path: Option<&OsString>) -> Result<Arc<Kernel>> {
        let mut cmd = command(&interp.program);
        cmd.args(&interp.args);
        match interp.flavor {
            Flavor::Python => {
                cmd.arg("-u").arg(driver);
                cmd.env("PYTHONUNBUFFERED", "1").env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
                if std::env::var_os("MPLBACKEND").is_none() {
                    // Figures render to PNG after each cell (no GUI windows).
                    cmd.env("MPLBACKEND", "Agg");
                }
            }
            Flavor::Node => {
                cmd.arg(driver);
            }
            _ => return Err(NotebookError::Invalid(format!("{} has no persistent kernel", interp.language))),
        }
        if let Some(p) = env_path {
            cmd.env("PATH", p);
        }
        cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|source| NotebookError::Spawn { program: interp.program.clone(), source })?;
        let tree = ProcessTree::attach(&child);
        let (Some(stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            tree.kill();
            return Err(NotebookError::Kernel("kernel pipes unavailable".into()));
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (ready_tx, ready_rx) = oneshot::channel();
        shared.lock().unwrap().ready = Some(ready_tx);
        let alive = Arc::new(AtomicBool::new(true));
        read_stdout(stdout, shared.clone(), alive.clone());
        read_stderr(stderr, shared.clone());

        let name = format!("{} kernel", interp.language.display_name());
        let version = match tokio::time::timeout(READY_TIMEOUT, ready_rx).await {
            Ok(Ok(Some(v))) => Some(v),
            Ok(Ok(None)) => None,
            Ok(Err(_)) => {
                // Exited before it was ready: report what it printed.
                tokio::time::sleep(Duration::from_millis(150)).await;
                tree.kill();
                let log = shared.lock().unwrap().log.trim().to_string();
                let detail = if log.is_empty() { String::new() } else { format!(":\n{log}") };
                return Err(NotebookError::Kernel(format!("The {name} ({}) exited while starting{detail}", interp.program.display())));
            }
            Err(_) => {
                tree.kill();
                return Err(NotebookError::Kernel(format!("The {name} didn't start within {} s", READY_TIMEOUT.as_secs())));
            }
        };
        if !alive.load(Ordering::SeqCst) {
            tree.kill();
            let log = shared.lock().unwrap().log.trim().to_string();
            return Err(NotebookError::Kernel(format!("The {name} exited while starting: {log}")));
        }
        Ok(Arc::new(Kernel {
            interpreter: interp.clone(),
            version: version.or_else(|| interp.version.clone()),
            stdin: tokio::sync::Mutex::new(stdin),
            tree,
            _child: child,
            shared,
            alive,
            exec_lock: tokio::sync::Mutex::new(()),
            redact: Mutex::new(BTreeSet::new()),
        }))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.tree.kill();
    }

    /// Adds secret values to this kernel's redaction set; returns the whole set.
    pub fn add_redactions(&self, values: &BTreeSet<String>) -> BTreeSet<String> {
        let mut r = self.redact.lock().unwrap();
        r.extend(values.iter().cloned());
        r.clone()
    }

    async fn write(&self, line: &str) -> std::io::Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await
    }

    /// Runs one cell (callers hold `exec_lock`): (exit code 0/1, interrupted, timed out).
    pub async fn exec(&self, id: u64, code: &str, cwd: &Path, env: &[(String, String)], ctx: &RunCtx) -> (Option<i32>, bool, bool) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.shared.lock().unwrap().current = Some((id, tx));
        let env: Map<String, Value> = env.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        let line = format!("{}\n", json!({ "t": "exec", "id": id, "code": code, "cwd": cwd.to_string_lossy(), "env": env }));
        if !self.is_alive() || self.write(&line).await.is_err() {
            self.shared.lock().unwrap().current = None;
            ctx.notice("The kernel stopped unexpectedly. Run the cell again to start a new one.");
            return (None, false, false);
        }
        let mut stop = ctx.stop_signal();
        let deadline = ctx.timeout.map(|t| Instant::now() + t);
        let (mut interrupted, mut timed_out, mut killed) = (false, false, false);
        let mut kill_at: Option<Instant> = None;
        let code = loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some(KMsg::Out { stderr, text }) => ctx.output(stderr, &text),
                    Some(KMsg::Display { data, result }) => ctx.display(data, result),
                    Some(KMsg::Error { ename, evalue, traceback }) => ctx.error(ename, evalue, traceback),
                    Some(KMsg::Done { ok, interrupted: i }) => {
                        interrupted |= i;
                        break Some(if ok { 0 } else { 1 });
                    }
                    Some(KMsg::Exited) | None => {
                        if !killed {
                            ctx.notice("The kernel exited (variables were cleared). The next run starts a new one.");
                        }
                        break None;
                    }
                },
                _ = stopped(&mut stop), if !interrupted => {
                    interrupted = true;
                    let _ = self.write("{\"t\":\"interrupt\"}\n").await;
                    kill_at = Some(Instant::now() + INTERRUPT_GRACE);
                }
                _ = tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() && !interrupted => {
                    interrupted = true;
                    timed_out = true;
                    let _ = self.write("{\"t\":\"interrupt\"}\n").await;
                    kill_at = Some(Instant::now() + INTERRUPT_GRACE);
                }
                _ = tokio::time::sleep_until(kill_at.unwrap_or_else(Instant::now)), if kill_at.is_some() => {
                    kill_at = None;
                    killed = true;
                    self.kill();
                    ctx.notice("The kernel didn't respond to the interrupt and was stopped; its variables were cleared. The next run starts a new kernel.");
                }
            }
        };
        self.shared.lock().unwrap().current = None;
        (code, interrupted, timed_out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_lines_route_to_the_current_cell() {
        let shared = Mutex::new(Shared::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        shared.lock().unwrap().current = Some((7, tx));
        handle_line(&shared, b"raw output\n");
        handle_line(&shared, format!("partial{MARK}{{\"t\":\"stream\",\"id\":7,\"name\":\"stderr\",\"text\":\"e\"}}\n").as_bytes());
        handle_line(&shared, format!("{MARK}{{\"t\":\"stream\",\"id\":6,\"name\":\"stdout\",\"text\":\"stale\"}}\n").as_bytes());
        handle_line(&shared, format!("{MARK}{{\"t\":\"done\",\"id\":7,\"ok\":true}}\n").as_bytes());
        let got: Vec<KMsg> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(matches!(&got[0], KMsg::Out { stderr: false, text } if text == "raw output\n"));
        assert!(matches!(&got[1], KMsg::Out { stderr: false, text } if text == "partial"));
        assert!(matches!(&got[2], KMsg::Out { stderr: true, text } if text == "e"));
        assert!(matches!(&got[3], KMsg::Done { ok: true, interrupted: false }));
        assert_eq!(got.len(), 4, "messages for another cell are dropped");
    }
}
