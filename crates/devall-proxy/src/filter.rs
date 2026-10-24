//! Exchange list filtering (the UI mirrors this logic in `features/inspector/filter.ts`).

use serde::{Deserialize, Serialize};

use crate::model::{ExchangeState, ExchangeSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    Json,
    Html,
    Script,
    Css,
    Image,
    Font,
    Media,
    Xml,
    Text,
    WebSocket,
    Other,
}

/// Classifies by MIME type, falling back to the path's extension.
pub fn content_kind(content_type: Option<&str>, path: &str) -> ContentKind {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    let ct = ct.split(';').next().unwrap_or("").trim();
    if !ct.is_empty() {
        if ct.contains("json") {
            return ContentKind::Json;
        }
        if ct.contains("html") {
            return ContentKind::Html;
        }
        if ct.contains("javascript") || ct.contains("ecmascript") || ct == "application/wasm" {
            return ContentKind::Script;
        }
        if ct == "text/css" {
            return ContentKind::Css;
        }
        if ct.starts_with("image/") {
            return ContentKind::Image;
        }
        if ct.starts_with("font/") || ct.contains("font-") || ct.contains("woff") {
            return ContentKind::Font;
        }
        if ct.starts_with("audio/") || ct.starts_with("video/") {
            return ContentKind::Media;
        }
        if ct.ends_with("xml") {
            return ContentKind::Xml;
        }
        if ct.starts_with("text/") {
            return ContentKind::Text;
        }
    }
    let p = path.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    let ext = p.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.contains('/')).unwrap_or("");
    match ext {
        "js" | "mjs" | "cjs" | "map" | "wasm" => ContentKind::Script,
        "css" => ContentKind::Css,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "ico" | "bmp" => ContentKind::Image,
        "woff" | "woff2" | "ttf" | "otf" | "eot" => ContentKind::Font,
        "mp4" | "webm" | "mp3" | "ogg" | "wav" | "m4a" | "m3u8" | "ts" => ContentKind::Media,
        "html" | "htm" => ContentKind::Html,
        "json" => ContentKind::Json,
        "xml" => ContentKind::Xml,
        _ => ContentKind::Other,
    }
}

/// Static assets hidden by "hide static": scripts, styles, images, fonts and media.
pub fn is_static(kind: ContentKind) -> bool {
    matches!(kind, ContentKind::Script | ContentKind::Css | ContentKind::Image | ContentKind::Font | ContentKind::Media)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExchangeFilter {
    /// Whitespace-separated terms matched case-insensitively against method, URL and status; all
    /// must match; a leading `-` excludes.
    pub text: String,
    pub methods: Vec<String>,
    /// First digit of the status (1–5); `0` = no response (pending, failed, tunnels).
    pub status_classes: Vec<u8>,
    /// Substring of the host.
    pub host: String,
    pub content_kinds: Vec<ContentKind>,
    pub hide_static: bool,
    pub only_paused: bool,
    /// Newest N after filtering.
    pub limit: Option<usize>,
}

impl ExchangeFilter {
    pub fn matches(&self, s: &ExchangeSummary) -> bool {
        if !self.methods.is_empty() && !self.methods.iter().any(|m| m.eq_ignore_ascii_case(&s.method)) {
            return false;
        }
        if !self.status_classes.is_empty() {
            let class = s.status.map_or(0, |st| (st / 100) as u8);
            if !self.status_classes.contains(&class) {
                return false;
            }
        }
        if !self.host.trim().is_empty() && !s.host.to_ascii_lowercase().contains(&self.host.trim().to_ascii_lowercase()) {
            return false;
        }
        let kind =
            if s.state == ExchangeState::WebSocket || s.ws_frames > 0 { ContentKind::WebSocket } else { content_kind(s.content_type.as_deref(), &s.path) };
        if !self.content_kinds.is_empty() && !self.content_kinds.contains(&kind) {
            return false;
        }
        if self.hide_static && is_static(kind) {
            return false;
        }
        if self.only_paused && s.paused.is_none() {
            return false;
        }
        let terms: Vec<String> = self.text.split_whitespace().map(str::to_lowercase).collect();
        if !terms.is_empty() {
            let hay = format!("{} {} {}", s.method, s.url, s.status.map(|v| v.to_string()).unwrap_or_default()).to_lowercase();
            for t in terms {
                match t.strip_prefix('-') {
                    Some(neg) if !neg.is_empty() => {
                        if hay.contains(neg) {
                            return false;
                        }
                    }
                    _ => {
                        if !hay.contains(&t) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Exchange;

    fn summary(method: &str, url: &str, status: Option<u16>, ct: Option<&str>) -> ExchangeSummary {
        let mut ex = Exchange::new(method, url, "HTTP/1.1", vec![]);
        if let Some(st) = status {
            ex.response = Some(crate::model::CapturedResponse {
                status: st,
                reason: String::new(),
                http_version: "HTTP/1.1".into(),
                headers: ct.map(|c| vec![("content-type".to_string(), c.to_string())]).unwrap_or_default(),
                body: Default::default(),
            });
        }
        ex.summary()
    }

    #[test]
    fn kinds() {
        assert_eq!(content_kind(Some("application/json; charset=utf-8"), "/x"), ContentKind::Json);
        assert_eq!(content_kind(None, "/static/app.min.js?v=3"), ContentKind::Script);
        assert_eq!(content_kind(Some("font/woff2"), "/f"), ContentKind::Font);
        assert_eq!(content_kind(None, "/api/users"), ContentKind::Other);
    }

    #[test]
    fn filters() {
        let a = summary("GET", "https://api.example.com/users?page=2", Some(200), Some("application/json"));
        let b = summary("POST", "https://cdn.example.com/app.js", Some(404), Some("application/javascript"));
        let c = summary("GET", "https://api.example.com/slow", None, None);
        let f = ExchangeFilter { hide_static: true, ..Default::default() };
        assert!(f.matches(&a) && !f.matches(&b) && f.matches(&c));
        let f = ExchangeFilter { status_classes: vec![4, 0], ..Default::default() };
        assert!(!f.matches(&a) && f.matches(&b) && f.matches(&c));
        let f = ExchangeFilter { text: "users -page=3".into(), methods: vec!["get".into()], host: "API.".into(), ..Default::default() };
        assert!(f.matches(&a) && !f.matches(&b));
        let f = ExchangeFilter { text: "-page=2".into(), ..Default::default() };
        assert!(!f.matches(&a));
        let f = ExchangeFilter { content_kinds: vec![ContentKind::Json], ..Default::default() };
        assert!(f.matches(&a) && !f.matches(&b));
    }
}
