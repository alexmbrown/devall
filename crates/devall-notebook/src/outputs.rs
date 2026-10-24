//! Streamed [`CellEvent`]s → nbformat 4 output objects (`stream`, `execute_result`,
//! `display_data`, `error`), the shape Jupyter and VS Code store in a cell's `outputs`.
//!
//! Text is stored ANSI-free with `\n` line endings and split into lines (Jupyter's on-disk
//! convention), which keeps saved outputs readable in diffs.

use serde_json::{Map, Value, json};

use crate::ansi::strip_ansi;
use crate::events::CellEvent;

/// MIME types whose payload is text (stored as a list of lines).
fn is_text_mime(mime: &str) -> bool {
    mime.starts_with("text/") || mime == "image/svg+xml" || mime == "application/javascript"
}

/// A string as an nbformat multiline string: a list of lines, each keeping its `\n`.
pub fn multiline(s: &str) -> Value {
    Value::Array(s.split_inclusive('\n').map(|l| Value::String(l.to_string())).collect())
}

/// Joins an nbformat multiline string (string or list of strings).
pub fn join_multiline(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    }
}

/// Folds events into nbformat outputs; consecutive chunks of the same stream are merged.
#[derive(Debug, Default)]
pub struct OutputCollector {
    execution_count: Option<u32>,
    outputs: Vec<Value>,
    /// (stream name, text) of the output being appended to.
    open_stream: Option<(String, String)>,
}

impl OutputCollector {
    pub fn new(execution_count: Option<u32>) -> Self {
        Self { execution_count, ..Self::default() }
    }

    fn close_stream(&mut self) {
        if let Some((name, text)) = self.open_stream.take()
            && !text.is_empty()
        {
            self.outputs.push(json!({ "output_type": "stream", "name": name, "text": multiline(&text) }));
        }
    }

    fn stream(&mut self, name: &str, plain: &str) {
        match &mut self.open_stream {
            Some((n, text)) if n == name => text.push_str(plain),
            _ => {
                self.close_stream();
                self.open_stream = Some((name.to_string(), plain.to_string()));
            }
        }
    }

    pub fn push(&mut self, event: &CellEvent) {
        match event {
            CellEvent::Stdout { plain, .. } => self.stream("stdout", plain),
            CellEvent::Stderr { plain, .. } => self.stream("stderr", plain),
            CellEvent::Display { data, result, .. } => {
                self.close_stream();
                let data = bundle_for_disk(data);
                if *result {
                    self.outputs.push(json!({ "output_type": "execute_result", "execution_count": self.execution_count, "data": data, "metadata": {} }));
                } else {
                    self.outputs.push(json!({ "output_type": "display_data", "data": data, "metadata": {} }));
                }
            }
            CellEvent::Error { ename, evalue, traceback, .. } => {
                self.close_stream();
                let tb: Vec<Value> = traceback.iter().map(|l| Value::String(strip_ansi(l))).collect();
                self.outputs.push(json!({ "output_type": "error", "ename": ename, "evalue": strip_ansi(evalue), "traceback": tb }));
            }
            CellEvent::Started { .. } | CellEvent::Notice { .. } | CellEvent::Exit { .. } => {}
        }
    }

    pub fn finish(mut self) -> Vec<Value> {
        self.close_stream();
        self.outputs
    }
}

fn bundle_for_disk(data: &Map<String, Value>) -> Map<String, Value> {
    data.iter()
        .map(|(mime, v)| {
            let v = match v {
                Value::String(s) if is_text_mime(mime) => multiline(&if mime == "text/plain" { strip_ansi(s) } else { s.clone() }),
                other => other.clone(),
            };
            (mime.clone(), v)
        })
        .collect()
}

/// Converts a whole event list (see [`OutputCollector`]).
pub fn to_nbformat(events: &[CellEvent], execution_count: Option<u32>) -> Vec<Value> {
    let mut c = OutputCollector::new(execution_count);
    for e in events {
        c.push(e);
    }
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(text: &str) -> CellEvent {
        CellEvent::Stdout { text: text.into(), plain: strip_ansi(text), at: 0 }
    }

    #[test]
    fn converts_events() {
        let mut data = Map::new();
        data.insert("text/plain".into(), Value::String("42".into()));
        data.insert("image/png".into(), Value::String("iVBOR".into()));
        let events = vec![
            out("a\n"),
            out("\u{1b}[32mb\u{1b}[0m\n"),
            CellEvent::Stderr { text: "warn\n".into(), plain: "warn\n".into(), at: 0 },
            out("c"),
            CellEvent::Display { data, result: true, at: 0 },
            CellEvent::Error { ename: "ValueError".into(), evalue: "bad".into(), traceback: vec!["\u{1b}[31mTraceback\u{1b}[0m".into()], at: 0 },
            CellEvent::Exit { code: Some(1), duration_ms: 3, interrupted: false, timed_out: false, at: 0 },
        ];
        let o = to_nbformat(&events, Some(7));
        assert_eq!(o.len(), 5);
        assert_eq!(o[0], json!({ "output_type": "stream", "name": "stdout", "text": ["a\n", "b\n"] }));
        assert_eq!(o[1]["name"], "stderr");
        assert_eq!(o[2]["text"], json!(["c"]));
        assert_eq!(o[3]["output_type"], "execute_result");
        assert_eq!(o[3]["execution_count"], 7);
        assert_eq!(o[3]["data"]["text/plain"], json!(["42"]));
        assert_eq!(o[3]["data"]["image/png"], "iVBOR");
        assert_eq!(o[4], json!({ "output_type": "error", "ename": "ValueError", "evalue": "bad", "traceback": ["Traceback"] }));
        assert_eq!(join_multiline(&o[0]["text"]), "a\nb\n");
    }
}
