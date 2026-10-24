//! HAR 1.2 → [`TrafficSample`]s (browser dev tools exports, the inspector's own HAR export).

use base64::Engine;
use serde_json::Value;

use crate::drift::TrafficSample;
use crate::error::{Error, Result};

fn header(headers: Option<&Value>, name: &str) -> Option<String> {
    headers?.as_array()?.iter().find(|h| h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name))).and_then(|h| h.get("value")?.as_str().map(str::to_string))
}

fn content_text(content: Option<&Value>) -> Option<String> {
    let c = content?;
    let text = c.get("text")?.as_str()?;
    if c.get("encoding").and_then(Value::as_str) == Some("base64") {
        let bytes = base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()?;
        return String::from_utf8(bytes).ok();
    }
    Some(text.to_string())
}

/// Parses a HAR document. Entries without a request URL are skipped; ids are 1-based entry
/// indexes.
pub fn parse_har(text: &str) -> Result<Vec<TrafficSample>> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| Error::Parse(format!("invalid HAR (JSON): {e}")))?;
    let entries = v.pointer("/log/entries").and_then(Value::as_array).ok_or_else(|| Error::Parse("not a HAR file (no log.entries)".into()))?;
    let mut out = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let Some(req) = e.get("request") else { continue };
        let Some(url) = req.get("url").and_then(Value::as_str) else { continue };
        let resp = e.get("response");
        let status = resp.and_then(|r| r.get("status")).and_then(Value::as_u64).filter(|s| *s > 0).map(|s| s as u16);
        let post = req.get("postData");
        out.push(TrafficSample {
            id: (i + 1).to_string(),
            url: url.to_string(),
            method: req.get("method").and_then(Value::as_str).unwrap_or("GET").to_string(),
            status,
            request_content_type: post.and_then(|p| p.get("mimeType")).and_then(Value::as_str).map(str::to_string).or_else(|| header(req.get("headers"), "content-type")),
            request_body: post.and_then(|p| p.get("text")).and_then(Value::as_str).map(str::to_string),
            response_content_type: resp
                .and_then(|r| r.pointer("/content/mimeType"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| header(resp.and_then(|r| r.get("headers")), "content-type")),
            response_body: content_text(resp.and_then(|r| r.get("content"))),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries() {
        let har = r#"{"log":{"version":"1.2","entries":[
            {"request":{"method":"POST","url":"https://api.example.com/v1/users","headers":[{"name":"Content-Type","value":"application/json"}],"postData":{"mimeType":"application/json","text":"{\"name\":\"x\"}"}},
             "response":{"status":201,"headers":[],"content":{"mimeType":"application/json","text":"eyJpZCI6MX0=","encoding":"base64"}}},
            {"request":{"method":"GET","url":"https://api.example.com/v1/users"},"response":{"status":0,"content":{}}},
            {"response":{"status":200}}
        ]}}"#;
        let s = parse_har(har).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].response_body.as_deref(), Some(r#"{"id":1}"#));
        assert_eq!(s[0].request_body.as_deref(), Some(r#"{"name":"x"}"#));
        assert_eq!(s[0].status, Some(201));
        assert_eq!(s[1].status, None);
        assert!(parse_har("{}").is_err());
    }
}
