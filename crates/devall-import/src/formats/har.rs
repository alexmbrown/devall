//! HAR 1.2 (browser dev tools, proxies). Static assets, CORS preflights, duplicates and browser
//! noise headers are dropped; cookies only on request.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::Value;

use devall_core::{Body, HttpRequestFile, KeyValue, MultipartPart};

use crate::ParseOptions;
use crate::detect::Format;
use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanFolder, PlanItem};
use crate::util::{Warnings, arr_at, body_from_text, default_name, parse_form_pairs, pretty_json, split_query, str_at, text_at};

/// Headers browsers add on their own; replaying them is noise at best and harmful at worst
/// (`If-None-Match` turns responses into empty 304s).
const NOISE_HEADERS: &[&str] = &[
    "accept-encoding",
    "accept-language",
    "connection",
    "host",
    "content-length",
    "user-agent",
    "referer",
    "priority",
    "dnt",
    "pragma",
    "cache-control",
    "te",
    "keep-alive",
    "upgrade-insecure-requests",
    "if-none-match",
    "if-modified-since",
    "purpose",
    "x-client-data",
];
const ASSET_TYPES: &[&str] = &["script", "stylesheet", "image", "font", "media", "manifest", "texttrack", "ping", "preflight", "cspviolationreport", "websocket", "eventsource", "signedexchange"];
const ASSET_EXTS: &[&str] = &[
    "js", "mjs", "cjs", "css", "png", "jpg", "jpeg", "gif", "svg", "ico", "webp", "avif", "bmp", "woff", "woff2", "ttf", "otf", "eot", "map", "mp4", "webm", "mp3",
    "wav", "ogg", "webmanifest",
];

pub(crate) fn parse(v: &Value, opts: &ParseOptions) -> Result<ImportPlan> {
    let entries = v.pointer("/log/entries").and_then(Value::as_array).ok_or_else(|| Error::Parse("not a HAR file (missing log.entries)".into()))?;
    let mut w = Warnings::default();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut by_host: IndexMap<String, Vec<HttpRequestFile>> = IndexMap::new();
    let (mut assets, mut dupes, mut preflights, mut cookies_dropped) = (0usize, 0usize, 0usize, 0usize);
    for e in entries {
        let Some(r) = e.get("request") else { continue };
        let method = str_at(r, "method").trim().to_uppercase();
        let url = str_at(r, "url").trim();
        if url.is_empty() || ["data:", "blob:", "chrome-extension:", "moz-extension:", "about:", "chrome:"].iter().any(|p| url.starts_with(p)) {
            continue;
        }
        let headers = arr_at(r, "headers");
        let is_preflight = method == "OPTIONS" && headers.iter().any(|h| str_at(h, "name").eq_ignore_ascii_case("access-control-request-method"));
        if is_preflight || str_at(e, "_resourceType") == "preflight" {
            preflights += 1;
            continue;
        }
        if !opts.include_assets && is_asset(e, url, &method) {
            assets += 1;
            continue;
        }
        if !seen.insert((method.clone(), url.to_string())) {
            dupes += 1;
            continue;
        }
        let (base, params) = split_query(url);
        let mut req = HttpRequestFile::new(default_name(&method, &base));
        req.method = if method.is_empty() { "GET".into() } else { method };
        req.url = base;
        req.params = params;
        for h in headers {
            let name = str_at(h, "name").trim();
            let lname = name.to_ascii_lowercase();
            if name.is_empty() || name.starts_with(':') || lname.starts_with("sec-") || NOISE_HEADERS.contains(&lname.as_str()) {
                continue;
            }
            if lname == "cookie" && !opts.include_cookies {
                cookies_dropped += 1;
                continue;
            }
            if !req.headers.iter().any(|x| x.name.eq_ignore_ascii_case(name)) {
                req.headers.push(KeyValue::new(name, text_at(h, "value")));
            }
        }
        if opts.include_cookies && !req.headers.iter().any(|h| h.name.eq_ignore_ascii_case("cookie")) {
            let jar: Vec<String> = arr_at(r, "cookies").iter().map(|c| format!("{}={}", str_at(c, "name"), str_at(c, "value"))).collect();
            if !jar.is_empty() {
                req.headers.push(KeyValue::new("Cookie", jar.join("; ")));
            }
        }
        if let Some(pd) = r.get("postData") {
            req.body = body(pd, &mut w, &req.name);
            if matches!(req.body, Body::Multipart { .. } | Body::Form { .. }) {
                req.headers.retain(|h| !h.name.eq_ignore_ascii_case("content-type") || matches!(req.body, Body::Form { .. }));
            }
        }
        let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
        by_host.entry(host).or_default().push(req);
    }

    let name = match by_host.keys().collect::<Vec<_>>().as_slice() {
        [one] if !one.is_empty() => format!("HAR {one}"),
        _ => "HAR import".to_string(),
    };
    let mut plan = ImportPlan::new(name, Format::Har);
    if by_host.len() <= 1 {
        plan.items = by_host.into_values().flatten().map(|r| PlanItem::Request(Box::new(r))).collect();
    } else {
        for (host, reqs) in by_host {
            let mut f = PlanFolder::new(if host.is_empty() { "Other".to_string() } else { host });
            f.items = reqs.into_iter().map(|r| PlanItem::Request(Box::new(r))).collect();
            plan.items.push(PlanItem::Folder(f));
        }
    }
    if assets > 0 {
        w.add(format!("Skipped {assets} static asset requests (scripts, styles, images, fonts)"));
    }
    if preflights > 0 {
        w.add(format!("Skipped {preflights} CORS preflight requests"));
    }
    if dupes > 0 {
        w.add(format!("Merged {dupes} duplicate requests (same method and URL)"));
    }
    if cookies_dropped > 0 {
        w.add(format!("Dropped the Cookie header of {cookies_dropped} requests (enable “include cookies” to keep them as secrets)"));
    }
    plan.warnings = w.finish();
    Ok(plan)
}

fn is_asset(e: &Value, url: &str, method: &str) -> bool {
    if let Some(t) = e.get("_resourceType").and_then(Value::as_str) {
        if ASSET_TYPES.contains(&t) {
            return true;
        }
        if matches!(t, "xhr" | "fetch" | "document") {
            return false;
        }
    }
    let mime = e.pointer("/response/content/mimeType").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    if ["image/", "font/", "text/css", "javascript", "video/", "audio/"].iter().any(|m| mime.contains(m)) {
        return true;
    }
    let path = url.split(['?', '#']).next().unwrap_or("");
    let last = path.rsplit('/').next().unwrap_or("");
    method == "GET" && last.rsplit_once('.').is_some_and(|(_, ext)| ASSET_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn body(pd: &Value, w: &mut Warnings, subject: &str) -> Body {
    let mime = str_at(pd, "mimeType");
    let lmime = mime.to_ascii_lowercase();
    let params = arr_at(pd, "params");
    let text = text_at(pd, "text");
    if str_at(pd, "encoding") == "base64" {
        w.about("Base64-encoded (binary) bodies were not imported", subject);
        return Body::None;
    }
    if lmime.contains("x-www-form-urlencoded") {
        return Body::Form {
            fields: if params.is_empty() { parse_form_pairs(text.trim()) } else { params.iter().map(|p| KeyValue::new(str_at(p, "name"), text_at(p, "value"))).collect() },
        };
    }
    if lmime.starts_with("multipart/") {
        if params.is_empty() {
            w.about("Multipart bodies without parsed fields were not imported", subject);
            return Body::None;
        }
        return Body::Multipart {
            parts: params
                .iter()
                .map(|p| {
                    let file = str_at(p, "fileName");
                    if !file.is_empty() {
                        w.about("Uploaded files are not stored in HAR files; re-select them", subject);
                    }
                    MultipartPart {
                        name: str_at(p, "name").to_string(),
                        value: if file.is_empty() { text_at(p, "value") } else { String::new() },
                        file: (!file.is_empty()).then(|| file.to_string()),
                        content_type: Some(str_at(p, "contentType").to_string()).filter(|c| !c.is_empty()),
                        enabled: true,
                    }
                })
                .collect(),
        };
    }
    let text = if lmime.contains("json") { pretty_json(&text) } else { text };
    body_from_text(text, Some(mime).filter(|m| !m.is_empty()))
}
