//! Live Kubernetes streams: resource watches, pod logs, pod exec.

use futures::io::AsyncBufReadExt as _;
use futures::{SinkExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, LogParams, TerminalSize};
use kube::runtime::watcher::{self, Event};
use kube::runtime::WatchStreamExt;
use kube::Client;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::resources::{KindRef, ResourceRow, columns_for, dyn_api, row_for, sort_rows, to_json};
use crate::error::{Error, Result};
use crate::stream::{default_shell, ExecEvent, ExecInput, LogLine, LogMsg, LogStream, Sink, Utf8Decoder, clip_line, pump_lines, split_timestamp};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WatchMsg {
    /// The full list (initial load and after every re-list).
    Reset { columns: Vec<String>, rows: Vec<ResourceRow> },
    Upsert { row: ResourceRow },
    Delete { uid: String },
    /// The watch failed; it retries with backoff.
    Error { message: String },
}

/// Watches `kind` (in `namespace`, or all namespaces) until the receiver goes away.
pub async fn watch(client: Client, kind: KindRef, namespace: Option<String>, label_selector: Option<String>, sink: Sink<WatchMsg>) {
    let api = dyn_api(client, &kind, namespace.as_deref());
    let mut cfg = watcher::Config::default();
    if let Some(sel) = label_selector.as_deref().filter(|s| !s.trim().is_empty()) {
        cfg = cfg.labels(sel);
    }
    let columns: Vec<String> = columns_for(&kind).into_iter().map(str::to_string).collect();
    let mut stream = watcher::watcher(api, cfg).default_backoff().boxed();
    let mut init: Vec<ResourceRow> = Vec::new();
    while let Some(ev) = stream.next().await {
        let alive = match ev {
            Ok(Event::Init) => {
                init.clear();
                true
            }
            Ok(Event::InitApply(o)) => {
                init.push(row_for(&kind, &to_json(&o)));
                true
            }
            Ok(Event::InitDone) => {
                let mut rows = std::mem::take(&mut init);
                sort_rows(&kind, &mut rows);
                sink(WatchMsg::Reset { columns: columns.clone(), rows })
            }
            Ok(Event::Apply(o)) => sink(WatchMsg::Upsert { row: row_for(&kind, &to_json(&o)) }),
            Ok(Event::Delete(o)) => sink(WatchMsg::Delete { uid: o.metadata.uid.unwrap_or_default() }),
            Err(e) => sink(WatchMsg::Error { message: crate::error::chain(&e) }),
        };
        if !alive {
            break;
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PodLogOptions {
    pub container: Option<String>,
    pub follow: bool,
    /// Logs of the previous (crashed) container instance.
    pub previous: bool,
    /// Last N lines (`None` = all).
    pub tail: Option<i64>,
    pub timestamps: bool,
    pub since_seconds: Option<i64>,
}

/// Streams a pod's logs to `sink` until the stream ends or the receiver goes away.
pub async fn pod_logs(client: Client, namespace: &str, pod: &str, opts: &PodLogOptions, sink: Sink<LogMsg>) {
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let lp = LogParams {
        container: opts.container.clone().filter(|c| !c.is_empty()),
        follow: opts.follow,
        previous: opts.previous,
        tail_lines: opts.tail,
        timestamps: opts.timestamps,
        since_seconds: opts.since_seconds,
        ..Default::default()
    };
    let reader = match api.log_stream(pod, &lp).await {
        Ok(r) => r,
        Err(e) => {
            sink(LogMsg::End { error: Some(Error::from(e).to_string()) });
            return;
        }
    };
    let timestamps = opts.timestamps;
    let lines = reader.lines().map(move |r| {
        r.map(|l| {
            let (ts, text) = if timestamps { split_timestamp(&l) } else { (None, l) };
            LogLine { ts, stream: LogStream::Stdout, text: clip_line(text) }
        })
        .map_err(|e| e.to_string())
    });
    pump_lines(Box::pin(lines), &sink).await;
}

/// The last `lines` lines of a pod's log (current container instance; `container` = the default
/// one when `None`), at most 64 KB.
pub async fn pod_log_tail(client: Client, namespace: &str, pod: &str, container: Option<&str>, lines: i64) -> Result<String> {
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let lp = LogParams { container: container.filter(|c| !c.is_empty()).map(str::to_string), tail_lines: Some(lines.clamp(1, 1000)), limit_bytes: Some(64 * 1024), ..Default::default() };
    Ok(api.logs(pod, &lp).await?)
}

/// Exit code from the exec status (`Failure` with an `ExitCode` cause, or 0 on `Success`).
fn exit_code(status: &k8s_openapi::apimachinery::pkg::apis::meta::v1::Status) -> (Option<i64>, Option<String>) {
    if status.status.as_deref() == Some("Success") {
        return (Some(0), None);
    }
    let code = status
        .details
        .as_ref()
        .and_then(|d| d.causes.as_ref())
        .and_then(|c| c.iter().find(|c| c.reason.as_deref() == Some("ExitCode")))
        .and_then(|c| c.message.as_deref())
        .and_then(|m| m.parse::<i64>().ok());
    let error = if code.is_some() { None } else { status.message.clone() };
    (code, error)
}

/// An interactive process in a pod container. `command` empty = a shell (bash, else sh).
#[allow(clippy::too_many_arguments)]
pub async fn pod_exec(
    client: Client,
    namespace: &str,
    pod: &str,
    container: Option<String>,
    command: Vec<String>,
    tty: bool,
    mut input: mpsc::Receiver<ExecInput>,
    sink: Sink<ExecEvent>,
) -> Result<()> {
    let api: Api<Pod> = Api::namespaced(client, namespace);
    let cmd: Vec<String> = if command.is_empty() { default_shell() } else { command };
    // A TTY merges stderr into stdout (the API rejects tty + stderr).
    let mut ap = AttachParams::default().stdin(true).stdout(true).stderr(!tty).tty(tty);
    if let Some(c) = container.filter(|c| !c.is_empty()) {
        ap = ap.container(c);
    }
    let mut proc = api.exec(pod, cmd, &ap).await?;
    let mut stdin = proc.stdin();
    let mut stdout = proc.stdout().ok_or_else(|| Error::Kube("exec: no stdout".into()))?;
    let mut stderr = proc.stderr();
    let mut resize = if tty { proc.terminal_size() } else { None };
    let status = proc.take_status();
    let mut out_dec = Utf8Decoder::default();
    let mut err_dec = Utf8Decoder::default();
    let mut buf = vec![0u8; 16 * 1024];
    let mut ebuf = vec![0u8; 8 * 1024];
    let mut input_open = true;
    loop {
        tokio::select! {
            n = stdout.read(&mut buf) => match n {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let data = out_dec.push(&buf[..n]);
                    if !data.is_empty() && !sink(ExecEvent::Output { data }) {
                        proc.abort();
                        return Ok(());
                    }
                }
            },
            n = async { match stderr.as_mut() { Some(e) => e.read(&mut ebuf).await, None => std::future::pending().await } } => match n {
                Ok(0) | Err(_) => stderr = None,
                Ok(n) => {
                    let data = err_dec.push(&ebuf[..n]);
                    if !data.is_empty() && !sink(ExecEvent::Output { data }) {
                        proc.abort();
                        return Ok(());
                    }
                }
            },
            msg = input.recv(), if input_open => match msg {
                Some(ExecInput::Data(bytes)) => {
                    if let Some(w) = stdin.as_mut()
                        && (w.write_all(&bytes).await.is_err() || w.flush().await.is_err())
                    {
                        stdin = None;
                    }
                }
                Some(ExecInput::Resize { cols, rows }) => {
                    if let Some(r) = resize.as_mut() {
                        let _ = r.send(TerminalSize { width: cols, height: rows }).await;
                    }
                }
                None => {
                    // The UI closed the session: closing stdin ends the shell.
                    if let Some(mut w) = stdin.take() {
                        let _ = w.shutdown().await;
                    }
                    input_open = false;
                }
            },
        }
    }
    let (code, error) = match status {
        Some(fut) => match fut.await {
            Some(s) => exit_code(&s),
            None => (None, None),
        },
        None => (None, None),
    };
    sink(ExecEvent::Exit { code, error });
    let _ = proc.join().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Status;

    #[test]
    fn exec_exit_codes() {
        let ok: Status = serde_json::from_value(serde_json::json!({ "status": "Success" })).unwrap();
        assert_eq!(exit_code(&ok), (Some(0), None));
        let failed: Status = serde_json::from_value(serde_json::json!({
            "status": "Failure", "message": "command terminated with non-zero exit code",
            "reason": "NonZeroExitCode", "details": { "causes": [ { "reason": "ExitCode", "message": "130" } ] }
        }))
        .unwrap();
        assert_eq!(exit_code(&failed), (Some(130), None));
        let other: Status = serde_json::from_value(serde_json::json!({ "status": "Failure", "message": "container not found" })).unwrap();
        assert_eq!(exit_code(&other), (None, Some("container not found".into())));
    }
}
