//! Incremental Server-Sent Events parser (WHATWG HTML §9.2), fed with arbitrary byte chunks.

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SseEvent {
    /// `message` when the server sent no `event:` field.
    pub event: String,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}

#[derive(Default)]
pub struct SseParser {
    buf: String,
    event: String,
    data: Vec<String>,
    id: Option<String>,
    retry: Option<u64>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find(['\n', '\r']) {
            let line: String = self.buf[..pos].to_string();
            let mut skip = 1;
            if self.buf[pos..].starts_with("\r\n") {
                skip = 2;
            }
            self.buf.drain(..pos + skip);
            if let Some(ev) = self.line(&line) {
                out.push(ev);
            }
        }
        out
    }

    fn line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            if self.data.is_empty() && self.event.is_empty() {
                return None;
            }
            let ev = SseEvent {
                event: if self.event.is_empty() { "message".into() } else { std::mem::take(&mut self.event) },
                data: std::mem::take(&mut self.data).join("\n"),
                id: self.id.clone(),
                retry: self.retry.take(),
            };
            return Some(ev);
        }
        if line.starts_with(':') {
            return None; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => self.data.push(value.to_string()),
            "id" if !value.contains('\0') => self.id = Some(value.to_string()),
            "retry" => self.retry = value.parse().ok(),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_split_chunks() {
        let mut p = SseParser::default();
        assert!(p.feed(b"data: hel").is_empty());
        let ev = p.feed(b"lo\ndata: world\n\n: ping\n\nevent: done\r\nid: 7\r\ndata: {\"a\":1}\r\n\r\n");
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].event, "message");
        assert_eq!(ev[0].data, "hello\nworld");
        assert_eq!(ev[1].event, "done");
        assert_eq!(ev[1].id.as_deref(), Some("7"));
        assert_eq!(ev[1].data, "{\"a\":1}");
    }
}
