//! Container log, stats and exec streams.

use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bollard::models::ContainerStatsResponse;
use bollard::query_parameters::{LogsOptions, StatsOptions};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use super::DockerClient;
use crate::error::Result;
use crate::stream::{default_shell, ExecEvent, ExecInput, LineSplitter, LogLine, LogMsg, LogStream, Sink, Utf8Decoder, clip_line, pump_lines, split_timestamp};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DockerLogOptions {
    pub follow: bool,
    /// Last N lines (`None` = all).
    pub tail: Option<u32>,
    pub timestamps: bool,
    /// Only lines newer than this many seconds.
    pub since_seconds: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSample {
    /// Unix ms when the sample was read.
    pub ts: i64,
    /// Percent of one CPU × online CPUs (like `docker stats`), e.g. 250.0 = 2.5 cores.
    pub cpu_percent: f64,
    pub online_cpus: u32,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub mem_percent: f64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub block_read: u64,
    pub block_write: u64,
    pub pids: u64,
}

/// The `docker stats` arithmetic: CPU from the delta against `precpu_stats`, memory minus page
/// cache (`inactive_file` on cgroup v2, `cache` on v1).
pub fn compute_stats(s: &ContainerStatsResponse) -> StatsSample {
    let cpu = s.cpu_stats.as_ref();
    let pre = s.precpu_stats.as_ref();
    let total = |c: Option<&bollard::models::ContainerCpuStats>| c.and_then(|c| c.cpu_usage.as_ref()).and_then(|u| u.total_usage).unwrap_or(0);
    let system = |c: Option<&bollard::models::ContainerCpuStats>| c.and_then(|c| c.system_cpu_usage).unwrap_or(0);
    let online = cpu
        .and_then(|c| c.online_cpus)
        .filter(|n| *n > 0)
        .or_else(|| cpu.and_then(|c| c.cpu_usage.as_ref()).and_then(|u| u.percpu_usage.as_ref()).map(|v| v.len() as u32))
        .filter(|n| *n > 0)
        .unwrap_or(1);
    let cpu_delta = total(cpu).saturating_sub(total(pre)) as f64;
    let sys_delta = system(cpu).saturating_sub(system(pre)) as f64;
    let cpu_percent = if cpu_delta > 0.0 && sys_delta > 0.0 { cpu_delta / sys_delta * online as f64 * 100.0 } else { 0.0 };

    let mem = s.memory_stats.as_ref();
    let usage = mem.and_then(|m| m.usage).unwrap_or(0);
    let cache = mem
        .and_then(|m| m.stats.as_ref())
        .and_then(|st| st.get("inactive_file").or_else(|| st.get("total_inactive_file")).or_else(|| st.get("cache")).copied())
        .unwrap_or(0);
    let mem_used = usage.saturating_sub(cache);
    let mem_limit = mem.and_then(|m| m.limit).unwrap_or(0);
    let mem_percent = if mem_limit > 0 { mem_used as f64 / mem_limit as f64 * 100.0 } else { 0.0 };

    let (mut net_rx, mut net_tx) = (0, 0);
    for n in s.networks.as_ref().map(|n| n.values().collect::<Vec<_>>()).unwrap_or_default() {
        net_rx += n.rx_bytes.unwrap_or(0);
        net_tx += n.tx_bytes.unwrap_or(0);
    }
    let (mut block_read, mut block_write) = (0, 0);
    for e in s.blkio_stats.as_ref().and_then(|b| b.io_service_bytes_recursive.as_ref()).map(|v| v.iter().collect::<Vec<_>>()).unwrap_or_default() {
        match e.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("read") => block_read += e.value.unwrap_or(0),
            Some("write") => block_write += e.value.unwrap_or(0),
            _ => {}
        }
    }
    StatsSample {
        ts: chrono::Utc::now().timestamp_millis(),
        cpu_percent: (cpu_percent * 100.0).round() / 100.0,
        online_cpus: online,
        mem_used,
        mem_limit,
        mem_percent: (mem_percent * 100.0).round() / 100.0,
        net_rx,
        net_tx,
        block_read,
        block_write,
        pids: s.pids_stats.as_ref().and_then(|p| p.current).unwrap_or(0),
    }
}

impl DockerClient {
    /// Streams a container's logs to `sink` until the stream ends or the receiver goes away.
    pub async fn stream_logs(&self, id: &str, opts: &DockerLogOptions, sink: Sink<LogMsg>) {
        let since = opts.since_seconds.map(|s| (chrono::Utc::now().timestamp() - s.max(0)) as i32).unwrap_or(0);
        let options = LogsOptions {
            follow: opts.follow,
            stdout: true,
            stderr: true,
            timestamps: opts.timestamps,
            tail: opts.tail.map(|n| n.to_string()).unwrap_or_else(|| "all".into()),
            since,
            until: 0,
        };
        let timestamps = opts.timestamps;
        let mut out = LineSplitter::default();
        let mut err = LineSplitter::default();
        let lines = self
            .raw()
            .logs(id, Some(options))
            .map(move |item| -> Vec<Result<LogLine, String>> {
                match item {
                    Ok(chunk) => {
                        let (split, stream) = match &chunk {
                            LogOutput::StdErr { .. } => (&mut err, LogStream::Stderr),
                            _ => (&mut out, LogStream::Stdout),
                        };
                        split
                            .push(chunk.as_ref())
                            .into_iter()
                            .map(|l| {
                                let (ts, text) = if timestamps { split_timestamp(&l) } else { (None, l) };
                                Ok(LogLine { ts, stream, text: clip_line(text) })
                            })
                            .collect()
                    }
                    Err(e) => vec![Err(crate::error::Error::from(e).to_string())],
                }
            })
            .flat_map(futures::stream::iter);
        pump_lines(Box::pin(lines), &sink).await;
    }

    /// Streams CPU/memory/network samples (about one per second) until the container stops or
    /// the receiver goes away. Returns the stream error, if any.
    pub async fn stream_stats(&self, id: &str, sink: Sink<StatsSample>) -> Result<()> {
        let mut stream = self.raw().stats(id, Some(StatsOptions { stream: true, one_shot: false }));
        while let Some(item) = stream.next().await {
            let s = item?;
            if !sink(compute_stats(&s)) {
                break;
            }
        }
        Ok(())
    }

    /// An interactive process inside a running container. `command` empty = a shell. Output goes
    /// to `sink` until the process exits; `input` carries keystrokes and terminal resizes.
    pub async fn exec(&self, id: &str, command: Vec<String>, tty: bool, mut input: mpsc::Receiver<ExecInput>, sink: Sink<ExecEvent>) -> Result<()> {
        let cmd = if command.is_empty() { default_shell() } else { command };
        let created = self
            .raw()
            .create_exec(
                id,
                CreateExecOptions::<String> {
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(tty),
                    env: Some(vec!["TERM=xterm-256color".into()]),
                    cmd: Some(cmd),
                    ..Default::default()
                },
            )
            .await?;
        let started = self.raw().start_exec(&created.id, Some(StartExecOptions { detach: false, tty, output_capacity: None })).await?;
        let StartExecResults::Attached { mut output, input: mut writer } = started else {
            sink(ExecEvent::Exit { code: None, error: Some("exec detached unexpectedly".into()) });
            return Ok(());
        };
        let mut decoder = Utf8Decoder::default();
        let mut input_open = true;
        let mut error: Option<String> = None;
        loop {
            tokio::select! {
                chunk = output.next() => match chunk {
                    Some(Ok(c)) => {
                        let data = decoder.push(c.as_ref());
                        if !data.is_empty() && !sink(ExecEvent::Output { data }) {
                            return Ok(());
                        }
                    }
                    Some(Err(e)) => { error = Some(crate::error::Error::from(e).to_string()); break; }
                    None => break,
                },
                msg = input.recv(), if input_open => match msg {
                    Some(ExecInput::Data(bytes)) => {
                        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                            input_open = false;
                        }
                    }
                    Some(ExecInput::Resize { cols, rows }) => {
                        if tty {
                            let _ = self.raw().resize_exec(&created.id, ResizeExecOptions { width: cols, height: rows }).await;
                        }
                    }
                    None => {
                        // The UI closed the session: end the process by closing stdin.
                        let _ = writer.shutdown().await;
                        input_open = false;
                    }
                },
            }
        }
        let code = self.raw().inspect_exec(&created.id).await.ok().and_then(|i| i.exit_code);
        sink(ExecEvent::Exit { code, error });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_arithmetic() {
        let raw = serde_json::json!({
            "read": "2026-10-21T10:00:01Z",
            "cpu_stats": { "cpu_usage": { "total_usage": 2_000_000_000u64 }, "system_cpu_usage": 20_000_000_000u64, "online_cpus": 4 },
            "precpu_stats": { "cpu_usage": { "total_usage": 1_000_000_000u64 }, "system_cpu_usage": 10_000_000_000u64, "online_cpus": 4 },
            "memory_stats": { "usage": 300_000_000u64, "limit": 1_000_000_000u64, "stats": { "inactive_file": 100_000_000u64 } },
            "networks": { "eth0": { "rx_bytes": 1000, "tx_bytes": 2000 }, "eth1": { "rx_bytes": 1, "tx_bytes": 2 } },
            "blkio_stats": { "io_service_bytes_recursive": [ { "major": 8, "minor": 0, "op": "read", "value": 4096 }, { "major": 8, "minor": 0, "op": "write", "value": 8192 } ] },
            "pids_stats": { "current": 7 }
        });
        let s: ContainerStatsResponse = serde_json::from_value(raw).unwrap();
        let sample = compute_stats(&s);
        // 1e9 / 1e10 * 4 cpus * 100 = 40 %
        assert_eq!(sample.cpu_percent, 40.0);
        assert_eq!(sample.online_cpus, 4);
        assert_eq!(sample.mem_used, 200_000_000);
        assert_eq!(sample.mem_percent, 20.0);
        assert_eq!((sample.net_rx, sample.net_tx), (1001, 2002));
        assert_eq!((sample.block_read, sample.block_write), (4096, 8192));
        assert_eq!(sample.pids, 7);
    }

    #[test]
    fn sparse_sample_falls_back_to_one_cpu() {
        let s: ContainerStatsResponse = serde_json::from_value(serde_json::json!({
            "cpu_stats": { "cpu_usage": { "total_usage": 5u64 }, "system_cpu_usage": 10u64 },
            "precpu_stats": {}
        }))
        .unwrap();
        let sample = compute_stats(&s);
        assert_eq!(sample.online_cpus, 1);
        assert_eq!(sample.cpu_percent, 50.0);
        assert_eq!(sample.mem_limit, 0);
        assert_eq!(sample.mem_percent, 0.0);
    }
}
