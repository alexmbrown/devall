//! Cloud Logging: query and live tail (polling `entries:list`, which works everywhere REST does;
//! the gRPC `TailLogEntries` stream would need HTTP/2 + protobuf). Read-only, real projects only.

use std::collections::HashSet;
use std::time::Duration;

use reqwest::Method;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Call, GcpClient, GcpTarget};
use crate::error::{Error, Result};
use crate::stream::Sink;

pub const API: &str = "https://logging.googleapis.com";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntryRow {
    pub ts: Option<String>,
    /// `DEFAULT`, `DEBUG`, `INFO`, `WARNING`, `ERROR`, …
    pub severity: String,
    /// One-line summary: text payload, a JSON payload's message, or the request line.
    pub text: String,
    /// Short log id (`stdout`, `cloudaudit.googleapis.com%2Factivity`, …).
    pub log: Option<String>,
    /// `k8s_container orders/api-7d9f` style resource label.
    pub resource: Option<String>,
    pub insert_id: String,
    /// The structured payload, when there is one.
    pub json: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LoggingTailMsg {
    Entries { entries: Vec<LogEntryRow> },
    Error { message: String },
}

fn resource_label(e: &Value) -> Option<String> {
    let ty = e.pointer("/resource/type").and_then(Value::as_str)?;
    let l = |k: &str| e.pointer(&format!("/resource/labels/{k}")).and_then(Value::as_str);
    let detail = match ty {
        "k8s_container" | "k8s_pod" => match (l("namespace_name"), l("pod_name")) {
            (Some(ns), Some(pod)) => Some(format!("{ns}/{pod}")),
            _ => None,
        },
        "cloud_run_revision" => l("service_name").map(str::to_string),
        "cloud_function" => l("function_name").map(str::to_string),
        "gce_instance" => l("instance_id").map(str::to_string),
        _ => None,
    };
    Some(match detail {
        Some(d) => format!("{ty} {d}"),
        None => ty.to_string(),
    })
}

/// Maps one API log entry to a row.
pub fn entry_row(e: &Value) -> LogEntryRow {
    let s = |p: &str| e.pointer(p).and_then(Value::as_str);
    let json_payload = e.get("jsonPayload").cloned();
    let text = if let Some(t) = s("/textPayload") {
        t.trim_end().to_string()
    } else if let Some(j) = &json_payload {
        ["message", "msg", "log", "textPayload", "error"]
            .iter()
            .find_map(|k| j.get(*k).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| j.to_string())
    } else if let Some(req) = e.get("httpRequest") {
        format!(
            "{} {} {}",
            req.get("requestMethod").and_then(Value::as_str).unwrap_or(""),
            req.get("requestUrl").and_then(Value::as_str).unwrap_or(""),
            req.get("status").map(|v| v.to_string()).unwrap_or_default()
        )
        .trim()
        .to_string()
    } else if let Some(p) = e.get("protoPayload") {
        let method = p.get("methodName").and_then(Value::as_str).unwrap_or("");
        let res = p.get("resourceName").and_then(Value::as_str).unwrap_or("");
        format!("{method} {res}").trim().to_string()
    } else {
        String::new()
    };
    LogEntryRow {
        ts: s("/timestamp").or_else(|| s("/receiveTimestamp")).map(str::to_string),
        severity: s("/severity").unwrap_or("DEFAULT").to_string(),
        text,
        log: s("/logName").map(|n| n.rsplit("/logs/").next().unwrap_or(n).to_string()),
        resource: resource_label(e),
        insert_id: s("/insertId").unwrap_or("").to_string(),
        json: json_payload.or_else(|| e.get("protoPayload").cloned()),
    }
}

/// `(<user filter>) AND timestamp >= "<since>"`.
pub fn tail_filter(user: &str, since: &str) -> String {
    let user = user.trim();
    if user.is_empty() { format!("timestamp >= \"{since}\"") } else { format!("({user}) AND timestamp >= \"{since}\"") }
}

impl GcpClient {
    async fn logging_page(&self, t: &GcpTarget, filter: &str, order: &str, page_size: u32, page_token: Option<&str>) -> Result<(Vec<Value>, Option<String>)> {
        if t.is_emulator() {
            return Err(Error::Invalid("Cloud Logging has no emulator; use a real project".into()));
        }
        let project = t.check_project()?;
        let mut body = json!({ "resourceNames": [format!("projects/{project}")], "orderBy": order, "pageSize": page_size.clamp(1, 1000) });
        if !filter.trim().is_empty() {
            body["filter"] = json!(filter);
        }
        if let Some(tok) = page_token {
            body["pageToken"] = json!(tok);
        }
        let v = self.call(Call { method: Method::POST, url: format!("{API}/v2/entries:list"), body: Some(body), target: t, emulator_bearer: None }).await?;
        let entries = v.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
        let next = v.get("nextPageToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        Ok((entries, next))
    }

    /// The newest `limit` entries matching `filter` (Logging query language), newest first.
    pub async fn logging_list(&self, t: &GcpTarget, filter: &str, limit: u32) -> Result<Vec<LogEntryRow>> {
        let (entries, _) = self.logging_page(t, filter, "timestamp desc", limit, None).await?;
        Ok(entries.iter().map(entry_row).collect())
    }

    /// Polls for new entries every `every` (backing off on quota errors) and streams them oldest
    /// first, starting `lookback` ago. Runs until the receiver goes away.
    pub async fn logging_tail(&self, t: &GcpTarget, filter: &str, every: Duration, lookback: Duration, sink: Sink<LoggingTailMsg>) {
        let start = chrono::Utc::now() - chrono::Duration::from_std(lookback).unwrap_or_else(|_| chrono::Duration::minutes(1));
        let mut since = start.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
        // insertIds already sent at exactly `since` (the filter is inclusive).
        let mut at_since: HashSet<String> = HashSet::new();
        let mut delay = every.max(Duration::from_secs(1));
        loop {
            let q = tail_filter(filter, &since);
            let mut fresh: Vec<LogEntryRow> = Vec::new();
            let mut token: Option<String> = None;
            let mut failed = None;
            for _ in 0..5 {
                match self.logging_page(t, &q, "timestamp asc", 1000, token.as_deref()).await {
                    Ok((entries, next)) => {
                        fresh.extend(entries.iter().map(entry_row));
                        token = next;
                        if token.is_none() {
                            break;
                        }
                    }
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = failed {
                let quota = matches!(&e, Error::Http { status: 429, .. });
                delay = if quota { (delay * 2).min(Duration::from_secs(60)) } else { delay };
                if !sink(LoggingTailMsg::Error { message: e.to_string() }) {
                    return;
                }
            } else {
                delay = every.max(Duration::from_secs(1));
            }
            let new: Vec<LogEntryRow> = fresh.into_iter().filter(|r| !(r.ts.as_deref() == Some(since.as_str()) && at_since.contains(&r.insert_id))).collect();
            if let Some(last) = new.last().and_then(|r| r.ts.clone()) {
                if last != since {
                    at_since.clear();
                    since = last;
                }
                for r in new.iter().filter(|r| r.ts.as_deref() == Some(since.as_str())) {
                    at_since.insert(r.insert_id.clone());
                }
            }
            if !sink(LoggingTailMsg::Entries { entries: new }) {
                return;
            }
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_from_entries() {
        let text = entry_row(&json!({
            "insertId": "i1", "timestamp": "2026-10-21T10:00:00.123Z", "severity": "ERROR", "textPayload": "boom\n",
            "logName": "projects/p/logs/stdout",
            "resource": { "type": "k8s_container", "labels": { "namespace_name": "orders", "pod_name": "api-7d9f" } }
        }));
        assert_eq!(text.text, "boom");
        assert_eq!(text.severity, "ERROR");
        assert_eq!(text.log.as_deref(), Some("stdout"));
        assert_eq!(text.resource.as_deref(), Some("k8s_container orders/api-7d9f"));

        let structured = entry_row(&json!({ "insertId": "i2", "jsonPayload": { "message": "order created", "orderId": 7 }, "resource": { "type": "cloud_run_revision", "labels": { "service_name": "web" } } }));
        assert_eq!(structured.text, "order created");
        assert_eq!(structured.json.unwrap()["orderId"], 7);
        assert_eq!(structured.severity, "DEFAULT");
        assert_eq!(structured.resource.as_deref(), Some("cloud_run_revision web"));

        let req = entry_row(&json!({ "insertId": "i3", "httpRequest": { "requestMethod": "GET", "requestUrl": "/health", "status": 200 } }));
        assert_eq!(req.text, "GET /health 200");
    }

    #[test]
    fn filters() {
        assert_eq!(tail_filter("", "T"), "timestamp >= \"T\"");
        assert_eq!(tail_filter("severity>=ERROR", "T"), "(severity>=ERROR) AND timestamp >= \"T\"");
    }

    #[tokio::test]
    async fn emulator_targets_are_rejected() {
        let client = GcpClient::new().unwrap();
        let err = client.logging_list(&GcpTarget::emulator("demo", "localhost:1"), "", 10).await.unwrap_err().to_string();
        assert!(err.contains("no emulator"));
    }
}
