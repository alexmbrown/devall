//! Incremental line splitting for streamed model output: Server-Sent Events (OpenAI-compatible,
//! Anthropic, Gemini) and newline-delimited JSON (Ollama's native API). Chunks may split lines
//! and UTF-8 sequences anywhere, so bytes are buffered until a full line is available.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    /// Complete lines (without `\n` / `\r\n`) contained in everything fed so far.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=pos).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
        }
        out
    }

    /// Whatever is left after the stream ended (a last line without a newline).
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        let s = String::from_utf8_lossy(&rest).trim_end_matches('\r').to_string();
        (!s.trim().is_empty()).then_some(s)
    }
}

#[derive(Default)]
pub struct SseParser {
    lines: LineBuffer,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for line in self.lines.feed(chunk) {
            self.line(&line, &mut out);
        }
        out
    }

    /// Flushes a final event that was not followed by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if let Some(line) = self.lines.finish() {
            self.line(&line, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn line(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if line.starts_with(':') {
            // comment / keep-alive
        } else if let Some(v) = line.strip_prefix("data:") {
            self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        } else if let Some(v) = line.strip_prefix("event:") {
            self.event = Some(v.trim().to_string());
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.data.is_empty() {
            self.event = None;
            return;
        }
        out.push(SseEvent { event: self.event.take(), data: std::mem::take(&mut self.data).join("\n") });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_events_across_chunks_and_utf8() {
        let mut p = SseParser::default();
        let text = "event: delta\ndata: {\"t\":\"héllo\"}\n\n: ping\n\ndata: [DONE]\n\n";
        let bytes = text.as_bytes();
        let mut events = Vec::new();
        // Feed one byte at a time to split the multi-byte 'é'.
        for b in bytes {
            events.extend(p.feed(std::slice::from_ref(b)));
        }
        events.extend(p.finish());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event.as_deref(), Some("delta"));
        assert_eq!(events[0].data, "{\"t\":\"héllo\"}");
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn ndjson_lines_and_trailing_line() {
        let mut l = LineBuffer::default();
        assert_eq!(l.feed(b"{\"a\":1}\r\n{\"b\""), vec!["{\"a\":1}"]);
        assert!(l.feed(b":2}").is_empty());
        assert_eq!(l.finish().as_deref(), Some("{\"b\":2}"));
    }
}
