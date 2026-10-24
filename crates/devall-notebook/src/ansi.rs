//! Terminal output helpers: ANSI escape stripping and a chunk decoder that never splits a UTF-8
//! character or an escape sequence across two events.

const ESC: char = '\u{1b}';

/// Removes ANSI/VT escape sequences (CSI colours and cursor movement, OSC titles and hyperlinks,
/// DCS/APC strings, charset selection) and control characters other than `\n`, `\r` and `\t`,
/// then normalizes `\r\n` to `\n`. Lone `\r` (progress bars) is kept.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ESC => match chars.next() {
                // CSI: parameters/intermediates up to a final byte in @..~
                Some('[') => {
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: up to BEL or ESC \
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == ESC && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // DCS, SOS, PM, APC: up to ESC \
                Some('P' | 'X' | '^' | '_') => {
                    while let Some(n) = chars.next() {
                        if n == ESC && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Charset designation takes one more character (ESC ( B).
                Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%') => {
                    chars.next();
                }
                _ => {}
            },
            // C1 CSI
            '\u{9b}' => {
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Normalizes line endings only (`\r\n` → `\n`).
pub fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Turns raw output chunks into text, holding back an incomplete UTF-8 character or escape
/// sequence at the end of a chunk until the next one arrives. Invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub struct StreamDecoder {
    pending: Vec<u8>,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> String {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(bytes);
        let mut out = String::with_capacity(buf.len());
        let mut start = 0;
        while start < buf.len() {
            match std::str::from_utf8(&buf[start..]) {
                Ok(s) => {
                    out.push_str(s);
                    start = buf.len();
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&buf[start..start + valid]).unwrap_or_default());
                    match e.error_len() {
                        Some(len) => {
                            out.push('\u{FFFD}');
                            start += valid + len;
                        }
                        None => {
                            self.pending = buf[start + valid..].to_vec();
                            start = buf.len();
                        }
                    }
                }
            }
        }
        // Hold back an unterminated escape sequence at the very end.
        if let Some(i) = out.rfind(ESC)
            && out.len() - i < 256
            && !escape_complete(&out[i..])
        {
            let mut tail = out.as_bytes()[i..].to_vec();
            tail.extend_from_slice(&self.pending);
            self.pending = tail;
            out.truncate(i);
        }
        out
    }

    /// Whatever is still held back, decoded lossily.
    pub fn flush(&mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        String::from_utf8_lossy(&rest).into_owned()
    }
}

/// Whether `s` (starting with ESC) holds a complete escape sequence.
fn escape_complete(s: &str) -> bool {
    let mut chars = s.chars().skip(1);
    match chars.next() {
        None => false,
        Some('[') => chars.any(|c| ('@'..='~').contains(&c)),
        Some(']') => s.contains('\u{7}') || s[1..].contains("\u{1b}\\"),
        Some('P' | 'X' | '^' | '_') => s[1..].contains("\u{1b}\\"),
        Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%') => chars.next().is_some(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes() {
        assert_eq!(strip_ansi("\u{1b}[1;31merror\u{1b}[0m: bad\r\n"), "error: bad\n");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}ok"), "ok");
        assert_eq!(strip_ansi("\u{1b}]8;;https://x.dev\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
        assert_eq!(strip_ansi("\u{1b}(Bplain\u{7}\u{8}"), "plain");
        assert_eq!(strip_ansi("10%\r20%\n"), "10%\r20%\n");
        assert_eq!(strip_ansi("tab\there ✓"), "tab\there ✓");
    }

    #[test]
    fn decoder_keeps_boundaries() {
        let mut d = StreamDecoder::new();
        let check = "✓".as_bytes();
        assert_eq!(d.push(&check[..1]), "");
        assert_eq!(d.push(&check[1..]), "✓");
        assert_eq!(d.push(b"a\x1b[3"), "a");
        assert_eq!(d.push(b"1mred"), "\u{1b}[31mred");
        assert_eq!(d.push(b"\xff!"), "\u{FFFD}!");
        assert_eq!(d.push(b"end\x1b"), "end");
        assert_eq!(d.flush(), "\u{1b}");
    }
}
