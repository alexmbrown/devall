//! cURL command → [`HttpRequestFile`]. Handles bash quoting (`'…'`, `"…"`, `$'…'`), line
//! continuations for bash (`\`), cmd (`^`) and PowerShell (`` ` ``), and the flags browsers and
//! API docs actually emit. Unknown flags are ignored rather than rejected.

use crate::error::{Error, Result};
use crate::model::*;

/// Flags that consume the next argument but don't affect the request.
const IGNORED_WITH_ARG: &[&str] = &[
    "-o", "--output", "-w", "--write-out", "--connect-timeout", "--retry", "--retry-delay", "--retry-max-time", "-x",
    "--proxy", "-U", "--proxy-user", "--cacert", "--capath", "-E", "--cert", "--key", "--cert-type", "--key-type", "-c",
    "--cookie-jar", "-D", "--dump-header", "-r", "--range", "--resolve", "--interface", "-K", "--config", "--limit-rate",
    "-y", "--speed-time", "-Y", "--speed-limit", "--max-redirs", "-T", "--upload-file", "--trace", "--trace-ascii",
];
/// Short flags that take an argument (for `-Xvalue` and combined short flags).
const SHORT_WITH_ARG: &[char] = &['X', 'H', 'd', 'u', 'A', 'e', 'b', 'F', 'm', 'o', 'w', 'x', 'U', 'E', 'c', 'D', 'r', 'K', 'y', 'Y', 'T'];

pub fn parse_curl(command: &str) -> Result<HttpRequestFile> {
    // Windows PowerShell prefixes piped text with a BOM.
    let tokens = tokenize(command.trim_start_matches('\u{feff}'))?;
    let mut it = tokens.into_iter().peekable();
    match it.peek() {
        Some(t) if t.eq_ignore_ascii_case("curl") || t.eq_ignore_ascii_case("curl.exe") => {
            it.next();
        }
        _ => {}
    }

    let mut method: Option<String> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut form: Vec<MultipartPart> = Vec::new();
    let mut json = false;
    let mut get = false;
    let mut head = false;
    let mut auth = Auth::Inherit;
    let mut settings = RequestSettings::default();

    // Expand `--flag=value` and combined/attached short flags into (flag, Option<value>).
    let mut args: Vec<String> = Vec::new();
    for t in it {
        if let Some(rest) = t.strip_prefix("--") {
            if let Some((f, v)) = rest.split_once('=') {
                args.push(format!("--{f}"));
                args.push(v.to_string());
                continue;
            }
            args.push(t);
        } else if t.len() > 2 && t.starts_with('-') && !t.starts_with("--") {
            let chars: Vec<char> = t[1..].chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let c = chars[i];
                args.push(format!("-{c}"));
                if SHORT_WITH_ARG.contains(&c) {
                    let rest: String = chars[i + 1..].iter().collect();
                    if !rest.is_empty() {
                        args.push(rest);
                    }
                    break;
                }
                i += 1;
            }
        } else {
            args.push(t);
        }
    }

    let mut a = args.into_iter();
    while let Some(flag) = a.next() {
        let mut val = |name: &str| a.next().ok_or_else(|| Error::Invalid(format!("cURL: {name} needs a value")));
        match flag.as_str() {
            "-X" | "--request" => method = Some(val(&flag)?.to_uppercase()),
            "-H" | "--header" => {
                let h = val(&flag)?;
                if let Some((k, v)) = h.split_once(':') {
                    headers.push(KeyValue::new(k.trim(), v.trim()));
                } else if let Some(k) = h.strip_suffix(';') {
                    headers.push(KeyValue::new(k.trim(), ""));
                }
            }
            "-d" | "--data" | "--data-ascii" | "--data-binary" | "--data-raw" => data.push(val(&flag)?),
            "--data-urlencode" => {
                let v = val(&flag)?;
                data.push(match v.split_once('=') {
                    Some((k, raw)) if !k.is_empty() => format!("{k}={}", url_encode(raw)),
                    Some((_, raw)) => url_encode(raw),
                    None => url_encode(&v),
                });
            }
            "--json" => {
                data.push(val(&flag)?);
                json = true;
            }
            "-F" | "--form" | "--form-string" => {
                let v = val(&flag)?;
                let (name, rest) = v.split_once('=').unwrap_or((v.as_str(), ""));
                let mut part = MultipartPart { name: name.to_string(), value: String::new(), file: None, content_type: None, enabled: true };
                let mut segs = rest.split(';');
                let first = segs.next().unwrap_or("");
                // `@file` uploads a file, `<file` sends its contents as the value; both reference a file.
                if flag != "--form-string" && (first.starts_with('@') || first.starts_with('<')) {
                    part.file = Some(first[1..].to_string());
                } else {
                    part.value = first.to_string();
                }
                for s in segs {
                    if let Some(t) = s.trim().strip_prefix("type=") {
                        part.content_type = Some(t.to_string());
                    }
                }
                form.push(part);
            }
            "-u" | "--user" => {
                let v = val(&flag)?;
                let (u, p) = v.split_once(':').unwrap_or((v.as_str(), ""));
                auth = Auth::Basic { username: u.to_string(), password: p.to_string() };
            }
            "-A" | "--user-agent" => headers.push(KeyValue::new("User-Agent", val(&flag)?)),
            "-e" | "--referer" => headers.push(KeyValue::new("Referer", val(&flag)?)),
            "-b" | "--cookie" => {
                let v = val(&flag)?;
                if v.contains('=') {
                    headers.push(KeyValue::new("Cookie", v));
                }
            }
            "-m" | "--max-time" => {
                if let Ok(secs) = val(&flag)?.parse::<f64>() {
                    settings.timeout_ms = (secs * 1000.0) as u64;
                }
            }
            "--url" => url = Some(val(&flag)?),
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            "-k" | "--insecure" => settings.verify_tls = false,
            f if IGNORED_WITH_ARG.contains(&f) => {
                let _ = val(f);
            }
            f if f.starts_with('-') && f.len() > 1 => {} // boolean flag we don't model (-s, -v, -L, --compressed, …)
            _ => {
                if url.is_none() {
                    url = Some(flag);
                }
            }
        }
    }

    let url = url.ok_or_else(|| Error::Invalid("cURL: no URL found".into()))?;
    let (base, mut params) = split_query(&url);

    let body_text = data.join("&");
    let mut body = Body::None;
    if get && !data.is_empty() {
        params.extend(parse_pairs(&body_text));
    } else if !form.is_empty() {
        body = Body::Multipart { parts: form };
    } else if !data.is_empty() {
        let ct = header_value(&headers, "content-type").map(|s| s.to_ascii_lowercase());
        let looks_json = serde_json::from_str::<serde_json::Value>(&body_text).is_ok_and(|v| v.is_object() || v.is_array());
        body = if json || ct.as_deref().is_some_and(|c| c.contains("json")) || (ct.is_none() && looks_json) {
            Body::Json { content: pretty_json(&body_text) }
        } else if ct.as_deref().is_none_or(|c| c.contains("x-www-form-urlencoded")) && is_form_pairs(&body_text) {
            Body::Form { fields: parse_pairs(&body_text) }
        } else if ct.as_deref().is_some_and(|c| c.contains("xml")) {
            Body::Xml { content: body_text }
        } else {
            Body::Text { content: body_text, content_type: None }
        };
        if json {
            for (k, v) in [("Content-Type", "application/json"), ("Accept", "application/json")] {
                if header_value(&headers, k).is_none() {
                    headers.push(KeyValue::new(k, v));
                }
            }
        }
    }

    let method = method.unwrap_or_else(|| {
        if head {
            "HEAD".into()
        } else if !body.is_none() {
            "POST".into()
        } else {
            "GET".into()
        }
    });

    let mut req = HttpRequestFile::new(default_name(&method, &base));
    req.method = method;
    req.url = base;
    req.params = params;
    req.headers = headers;
    req.auth = auth;
    req.body = body;
    req.settings = settings;
    Ok(req)
}

fn default_name(method: &str, url: &str) -> String {
    let path = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let path = path.find('/').map(|i| &path[i..]).unwrap_or("/");
    format!("{method} {path}")
}

fn header_value<'a>(headers: &'a [KeyValue], name: &str) -> Option<&'a str> {
    headers.iter().find(|h| h.name.eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

fn pretty_json(s: &str) -> String {
    serde_json::from_str::<serde_json::Value>(s).ok().and_then(|v| serde_json::to_string_pretty(&v).ok()).unwrap_or_else(|| s.to_string())
}

fn is_form_pairs(s: &str) -> bool {
    !s.is_empty() && s.split('&').all(|p| p.contains('=') && !p.contains(char::is_whitespace))
}

/// Splits `?a=1&b=2` off a URL into decoded params; keeps the fragment-free base.
pub fn split_query(url: &str) -> (String, Vec<KeyValue>) {
    let url = url.split('#').next().unwrap_or(url);
    match url.split_once('?') {
        Some((base, q)) => (base.to_string(), parse_pairs(q)),
        None => (url.to_string(), Vec::new()),
    }
}

fn parse_pairs(q: &str) -> Vec<KeyValue> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            KeyValue::new(url_decode(k), url_decode(v))
        })
        .collect()
}

fn url_decode(s: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Shell-like tokenizer covering bash, cmd and PowerShell "copy as cURL" output.
pub fn tokenize(input: &str) -> Result<Vec<String>> {
    // cmd-style copies (Chrome "Copy as cURL (cmd)") escape every special char with ^. cmd removes
    // the carets before curl sees the line, so do the same, then parse like a POSIX shell.
    let cmd_style = input.contains("^\"") || input.contains("^\n") || input.contains("^\r\n");
    let input = if cmd_style { strip_cmd_carets(input) } else { input.to_string() };
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            '\\' | '`' => {
                // Line continuation (bash `\`, PowerShell `` ` ``)?
                let mut j = i + 1;
                if chars.get(j) == Some(&'\r') {
                    j += 1;
                }
                if chars.get(j) == Some(&'\n') {
                    i = j;
                } else if c == '`' {
                    // A PowerShell backtick only matters before a newline; otherwise literal.
                    cur.push(c);
                    has = true;
                } else if let Some(&n) = chars.get(i + 1) {
                    cur.push(n);
                    has = true;
                    i += 1;
                }
            }
            '\'' => {
                has = true;
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    cur.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(Error::Invalid("cURL: unterminated ' quote".into()));
                }
            }
            '$' if chars.get(i + 1) == Some(&'\'') => {
                has = true;
                i += 2;
                while i < chars.len() && chars[i] != '\'' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        cur.push(match chars[i] {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    } else {
                        cur.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(Error::Invalid("cURL: unterminated $' quote".into()));
                }
            }
            '"' => {
                has = true;
                i += 1;
                loop {
                    let Some(&ch) = chars.get(i) else {
                        return Err(Error::Invalid("cURL: unterminated \" quote".into()));
                    };
                    if ch == '"' {
                        break;
                    }
                    if ch == '\\'
                        && let Some(&n) = chars.get(i + 1)
                    {
                        if matches!(n, '"' | '\\' | '$' | '`') {
                            cur.push(n);
                            i += 2;
                            continue;
                        }
                        if n == '\n' {
                            i += 2;
                            continue;
                        }
                    }
                    cur.push(ch);
                    i += 1;
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
        i += 1;
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// cmd.exe semantics: `^<newline>` continues the line, `^X` is a literal X.
fn strip_cmd_carets(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '^' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            Some('\n') | None => {}
            Some(n) => out.push(n),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_get_with_query() {
        let r = parse_curl("curl 'https://api.example.com/users?page=2&q=a%20b' -H 'Accept: application/json'").unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.url, "https://api.example.com/users");
        assert_eq!(r.params[0], KeyValue::new("page", "2"));
        assert_eq!(r.params[1].value, "a b");
        assert_eq!(r.headers[0].name, "Accept");
        assert_eq!(r.name, "GET /users");
    }

    #[test]
    fn post_json_multiline() {
        let cmd = "curl -X POST https://x.io/items \\\n  -H \"Content-Type: application/json\" \\\n  -d '{\"a\":1}'";
        let r = parse_curl(cmd).unwrap();
        assert_eq!(r.method, "POST");
        assert!(matches!(&r.body, Body::Json { content } if content.contains("\"a\": 1")));
    }

    #[test]
    fn implicit_post_form_and_basic_auth() {
        let r = parse_curl("curl -u user:pa:ss -d a=1 -d b=two https://x.io").unwrap();
        assert_eq!(r.method, "POST");
        assert!(matches!(&r.body, Body::Form { fields } if fields.len() == 2));
        assert_eq!(r.auth, Auth::Basic { username: "user".into(), password: "pa:ss".into() });
    }

    #[test]
    fn combined_and_attached_flags() {
        let r = parse_curl("curl -sSLk -XPUT -HX-Id:7 --url=https://x.io/a --json '{\"x\":true}'").unwrap();
        assert_eq!(r.method, "PUT");
        assert!(!r.settings.verify_tls);
        assert_eq!(r.headers[0], KeyValue::new("X-Id", "7"));
        assert!(r.headers.iter().any(|h| h.name == "Accept"));
    }

    #[test]
    fn get_flag_moves_data_to_query() {
        let r = parse_curl("curl -G https://x.io/s --data-urlencode 'q=hello world'").unwrap();
        assert_eq!(r.method, "GET");
        assert_eq!(r.params[0].value, "hello world");
        assert!(r.body.is_none());
    }

    #[test]
    fn multipart() {
        let r = parse_curl("curl -F 'file=@./a.png;type=image/png' -F name=x https://x.io/up").unwrap();
        let Body::Multipart { parts } = r.body else { panic!() };
        assert_eq!(parts[0].file.as_deref(), Some("./a.png"));
        assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));
        assert_eq!(parts[1].value, "x");
    }

    #[test]
    fn cmd_style_copy() {
        let cmd = "curl ^\"https://x.io/a^\" ^\r\n  -H ^\"accept: */*^\" ^\r\n  --data-raw ^\"^{^\\^\"k^\\^\":1^}^\"";
        let r = parse_curl(cmd).unwrap();
        assert_eq!(r.url, "https://x.io/a");
        assert_eq!(r.headers[0].value, "*/*");
    }

    #[test]
    fn ansi_c_quoting() {
        let r = parse_curl("curl $'https://x.io/a' --data-raw $'line1\\nline2'").unwrap();
        assert!(matches!(&r.body, Body::Text { content, .. } if content == "line1\nline2"));
    }

    #[test]
    fn leading_bom_is_ignored() {
        let r = parse_curl("\u{feff}curl https://x.io/a").unwrap();
        assert_eq!(r.url, "https://x.io/a");
    }

    #[test]
    fn errors() {
        assert!(parse_curl("curl -H 'A: b'").is_err());
        assert!(parse_curl("curl 'https://x").is_err());
    }
}
