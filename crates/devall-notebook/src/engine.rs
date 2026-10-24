//! The execution API: start a cell, stream its events, interrupt it, restart or shut down a
//! notebook's kernels.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::sync::{mpsc, watch};

use crate::ansi::{normalize_newlines, strip_ansi};
use crate::error::{NotebookError, Result};
use crate::events::{CellEvent, CellResult};
use crate::interp::{Interpreter, Interpreters};
use crate::kernel::{Kernel, NODE_DRIVER, PYTHON_DRIVER};
use crate::outputs::OutputCollector;
use crate::{Language, now_ms, shell};

/// How to run one cell.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Caller-chosen id (for [`Engine::interrupt`]); generated when `None`.
    pub run_id: Option<String>,
    /// Working directory (usually the project root).
    pub cwd: PathBuf,
    /// Extra environment variables (see [`crate::env::prepare`]).
    pub env: Vec<(String, String)>,
    /// Interrupt the cell after this long.
    pub timeout: Option<Duration>,
    /// Kernel scope: JavaScript and Python cells with the same key share one process (and
    /// variables). Use one key per notebook, e.g. `<project root>|<notebook path>`.
    pub kernel_key: String,
    /// Values to mask in the output (secrets the cell received).
    pub redact: BTreeSet<String>,
    /// `execution_count` for an `execute_result` output.
    pub execution_count: Option<u32>,
}

/// Per-run plumbing shared by the shell runner and kernels.
#[derive(Clone)]
pub(crate) struct RunCtx {
    pub run_id: String,
    tx: mpsc::UnboundedSender<CellEvent>,
    stop: watch::Receiver<bool>,
    redact: Arc<BTreeSet<String>>,
    pub timeout: Option<Duration>,
}

impl RunCtx {
    fn mask(&self, text: &str) -> String {
        if self.redact.is_empty() { text.to_string() } else { devall_core::vars::redact_values(text, &self.redact) }
    }

    pub fn emit(&self, e: CellEvent) {
        let _ = self.tx.send(e);
    }

    pub fn output(&self, stderr: bool, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = self.mask(text);
        let plain = normalize_newlines(&strip_ansi(&text));
        let at = now_ms();
        self.emit(if stderr { CellEvent::Stderr { text, plain, at } } else { CellEvent::Stdout { text, plain, at } });
    }

    pub fn display(&self, data: Map<String, Value>, result: bool) {
        let data = data
            .into_iter()
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) if !k.starts_with("image/") || k == "image/svg+xml" => Value::String(self.mask(&s)),
                    other => other,
                };
                (k, v)
            })
            .collect();
        self.emit(CellEvent::Display { data, result, at: now_ms() });
    }

    pub fn error(&self, ename: String, evalue: String, traceback: Vec<String>) {
        let traceback = traceback.iter().map(|l| self.mask(l)).collect();
        self.emit(CellEvent::Error { ename, evalue: self.mask(&evalue), traceback, at: now_ms() });
    }

    pub fn notice(&self, text: &str) {
        self.emit(CellEvent::Notice { text: text.to_string(), at: now_ms() });
    }

    pub fn stop_signal(&self) -> watch::Receiver<bool> {
        self.stop.clone()
    }
}

/// Resolves once a stop (interrupt) was requested; never if the run's controller went away.
pub(crate) async fn stopped(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|v| *v).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// A started cell: stream its events with [`RunHandle::next`] or fold them with
/// [`RunHandle::finish`]. The last event is always [`CellEvent::Exit`].
pub struct RunHandle {
    pub run_id: String,
    pub language: Language,
    pub interpreter: Interpreter,
    execution_count: Option<u32>,
    events: mpsc::UnboundedReceiver<CellEvent>,
}

impl RunHandle {
    pub async fn next(&mut self) -> Option<CellEvent> {
        self.events.recv().await
    }

    /// Drains the run, calling `on_event` for every event, and returns the outcome with nbformat
    /// outputs.
    pub async fn finish(mut self, mut on_event: impl FnMut(&CellEvent)) -> CellResult {
        let mut collector = OutputCollector::new(self.execution_count);
        let mut r = CellResult {
            run_id: self.run_id.clone(),
            language: self.language,
            interpreter: Some(self.interpreter.label()),
            exit_code: None,
            ok: false,
            interrupted: false,
            timed_out: false,
            duration_ms: 0,
            outputs: Vec::new(),
        };
        let mut raised = false;
        while let Some(e) = self.events.recv().await {
            on_event(&e);
            collector.push(&e);
            match &e {
                CellEvent::Error { .. } => raised = true,
                CellEvent::Exit { code, duration_ms, interrupted, timed_out, .. } => {
                    r.exit_code = *code;
                    r.duration_ms = *duration_ms;
                    r.interrupted = *interrupted;
                    r.timed_out = *timed_out;
                }
                _ => {}
            }
        }
        r.ok = r.exit_code == Some(0) && !raised && !r.interrupted;
        r.outputs = collector.finish();
        r
    }
}

enum Job {
    Shell(Box<shell::Spawned>),
    Kernel(Arc<Kernel>),
}

/// Runs cells. Cheap to share (`Arc<Engine>`); all methods take `&self`.
pub struct Engine {
    interpreters: Interpreters,
    scratch: PathBuf,
    kernels: tokio::sync::Mutex<HashMap<(String, Language), Arc<Kernel>>>,
    runs: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    exec_ids: AtomicU64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(std::env::temp_dir().join("devall-notebook"))
    }
}

impl Engine {
    /// `scratch` holds cell scripts (deleted after each run) and the kernel driver scripts; use a
    /// per-user directory such as `<app data>/notebook-tmp`.
    pub fn new(scratch: PathBuf) -> Self {
        Self { interpreters: Interpreters::new(), scratch, kernels: Default::default(), runs: Default::default(), exec_ids: AtomicU64::new(0) }
    }

    pub fn interpreters(&self) -> &Interpreters {
        &self.interpreters
    }

    fn driver(&self, language: Language) -> Result<PathBuf> {
        let (name, body) = match language {
            Language::Javascript => ("devall_node_kernel.js", NODE_DRIVER),
            Language::Python => ("devall_python_kernel.py", PYTHON_DRIVER),
            _ => return Err(NotebookError::Invalid(format!("{language} has no kernel"))),
        };
        let dir = self.scratch.join("kernels");
        let path = dir.join(name);
        // Rewritten when different, so an edited/stale copy is never run.
        devall_core::fsutil::atomic_write(&path, body.as_bytes())?;
        Ok(path)
    }

    async fn kernel(&self, key: &str, language: Language, interp: &Interpreter, cwd: &Path) -> Result<Arc<Kernel>> {
        let mut map = self.kernels.lock().await;
        let k = (key.to_string(), language);
        if let Some(existing) = map.get(&k) {
            if existing.is_alive() && existing.interpreter.program == interp.program {
                return Ok(existing.clone());
            }
            if let Some(old) = map.remove(&k) {
                old.kill();
            }
        }
        let driver = self.driver(language)?;
        let kernel = Kernel::start(interp, &driver, cwd, self.interpreters.env_path().as_ref()).await?;
        map.insert(k, kernel.clone());
        Ok(kernel)
    }

    /// Starts `code` in `language`. Fails fast when the interpreter is missing or can't start;
    /// everything after that (output, errors, exit) arrives as events.
    pub async fn start(&self, code: &str, language: Language, opts: RunOptions) -> Result<RunHandle> {
        let interp = self.interpreters.get(language).await?;
        if !opts.cwd.is_dir() {
            return Err(NotebookError::Invalid(format!("working directory not found: {}", opts.cwd.display())));
        }
        let job = if language.is_persistent() {
            Job::Kernel(self.kernel(&opts.kernel_key, language, &interp, &opts.cwd).await?)
        } else {
            Job::Shell(Box::new(shell::spawn(&interp, code, &opts.cwd, &opts.env, self.interpreters.env_path().as_ref(), &self.scratch)?))
        };
        let run_id = opts.run_id.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| ulid::Ulid::generate().to_string());
        let (stop_tx, stop_rx) = watch::channel(false);
        self.runs.lock().unwrap().insert(run_id.clone(), stop_tx);
        let redact = match &job {
            Job::Kernel(k) => k.add_redactions(&opts.redact),
            Job::Shell(_) => opts.redact.clone(),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = RunCtx { run_id: run_id.clone(), tx, stop: stop_rx, redact: Arc::new(redact), timeout: opts.timeout };
        let version = match &job {
            Job::Kernel(k) => k.version.clone(),
            Job::Shell(_) => interp.version.clone(),
        };
        ctx.emit(CellEvent::Started {
            run_id: run_id.clone(),
            language,
            interpreter: interp.name.clone(),
            program: interp.program.to_string_lossy().into_owned(),
            version,
            at: now_ms(),
        });
        if let Some(note) = &interp.note {
            ctx.notice(note);
        }

        let runs = self.runs.clone();
        let exec_id = self.exec_ids.fetch_add(1, Ordering::SeqCst) + 1;
        let (code, cwd, env) = (code.to_string(), opts.cwd.clone(), opts.env.clone());
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            let (exit, interrupted, timed_out) = match job {
                Job::Shell(sp) => shell::run(*sp, &ctx).await,
                Job::Kernel(k) => {
                    // Wait for earlier cells of this kernel; an interrupt while queued cancels.
                    let mut stop = ctx.stop_signal();
                    let guard = tokio::select! {
                        g = k.exec_lock.lock() => Some(g),
                        _ = stopped(&mut stop) => None,
                    };
                    match guard {
                        Some(_g) => k.exec(exec_id, &code, &cwd, &env, &ctx).await,
                        None => (None, true, false),
                    }
                }
            };
            ctx.emit(CellEvent::Exit { code: exit, duration_ms: started.elapsed().as_millis() as u64, interrupted, timed_out, at: now_ms() });
            runs.lock().unwrap().remove(&ctx.run_id);
        });
        Ok(RunHandle { run_id, language, interpreter: interp, execution_count: opts.execution_count, events: rx })
    }

    /// Ctrl-C for a running (or queued) cell. Returns false if no such run is in progress.
    pub fn interrupt(&self, run_id: &str) -> bool {
        match self.runs.lock().unwrap().get(run_id) {
            Some(tx) => {
                let _ = tx.send(true);
                true
            }
            None => false,
        }
    }

    /// Runs in progress.
    pub fn running(&self) -> Vec<String> {
        self.runs.lock().unwrap().keys().cloned().collect()
    }

    /// Stops the kernels of `kernel_key` (variables are lost); the next cell starts fresh ones.
    /// Returns how many kernels were stopped.
    pub async fn restart(&self, kernel_key: &str) -> usize {
        let mut map = self.kernels.lock().await;
        let keys: Vec<_> = map.keys().filter(|(k, _)| k == kernel_key).cloned().collect();
        for k in &keys {
            if let Some(kernel) = map.remove(k) {
                kernel.kill();
            }
        }
        keys.len()
    }

    /// Same as [`Engine::restart`]; named for closing a notebook.
    pub async fn shutdown(&self, kernel_key: &str) -> usize {
        self.restart(kernel_key).await
    }

    /// Stops every kernel and interrupts every run (app exit).
    pub async fn shutdown_all(&self) {
        for tx in self.runs.lock().unwrap().values() {
            let _ = tx.send(true);
        }
        let mut map = self.kernels.lock().await;
        for (_, k) in map.drain() {
            k.kill();
        }
    }

    /// Live kernels of a key: (language, interpreter label).
    pub async fn kernels(&self, kernel_key: &str) -> Vec<(Language, String)> {
        let map = self.kernels.lock().await;
        let mut out: Vec<_> = map
            .iter()
            .filter(|((k, _), kernel)| k == kernel_key && kernel.is_alive())
            .map(|((_, l), kernel)| (*l, format!("{} {}", kernel.interpreter.name, kernel.version.clone().unwrap_or_default()).trim().to_string()))
            .collect();
        out.sort();
        out
    }
}
