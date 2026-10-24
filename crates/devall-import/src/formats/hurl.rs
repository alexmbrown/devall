//! Hurl files. Requests, sections and bodies map directly; `HTTP` status lines, `[Asserts]` and
//! `[Captures]` become tests and captures where an equivalent exists (the rest is kept in docs).

use devall_core::{AssertOp, AssertTarget, Assertion, Auth, Body, Capture, CaptureSource, HttpRequestFile, KeyValue, MultipartPart};

use crate::detect::{Format, is_hurl_marker, is_request_line};
use crate::plan::{ImportPlan, PlanItem};
use crate::util::{Warnings, body_from_text, default_name, split_query};

const SECTIONS: &[&str] = &[
    "[QueryStringParams]",
    "[Query]",
    "[FormParams]",
    "[Form]",
    "[MultipartFormData]",
    "[Multipart]",
    "[Cookies]",
    "[BasicAuth]",
    "[Options]",
    "[Asserts]",
    "[Captures]",
];

pub(crate) fn parse(text: &str, name: &str) -> ImportPlan {
    let mut plan = ImportPlan::new(name, Format::Hurl);
    let mut w = Warnings::default();
    for entry in entries(text) {
        plan.items.push(PlanItem::Request(Box::new(entry_to_request(&entry, &mut w))));
    }
    plan.warnings = w.finish();
    plan
}

/// Groups lines into entries; a new entry starts at each request line outside a ``` block.
fn entries(text: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut in_fence = false;
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim_end();
        if !in_fence && line.trim_start().starts_with('#') {
            continue;
        }
        if !in_fence && is_request_line(line) {
            out.push(Vec::new());
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if let Some(e) = out.last_mut() {
            e.push(line.to_string());
        }
    }
    out
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some(o) => out.push(o),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        out
    } else {
        // Unquoted key/value text escapes `#` (comment start) and `\\`.
        v.replace("\\#", "#").replace("\\\\", "\\")
    }
}

fn key_value(l: &str) -> Option<(String, String)> {
    let (k, v) = l.split_once(':')?;
    let k = k.trim();
    (!k.is_empty() && !k.contains(char::is_whitespace)).then(|| (unquote(k), unquote(v)))
}

fn entry_to_request(lines: &[String], w: &mut Warnings) -> HttpRequestFile {
    let first = lines.first().map(String::as_str).unwrap_or("");
    let (method, url) = first.split_once(char::is_whitespace).unwrap_or((first, ""));
    let (base, params) = split_query(url.trim());
    let mut req = HttpRequestFile::new(default_name(method, &base));
    req.method = method.to_string();
    req.url = base;
    req.params = params;
    // Hurl doesn't follow redirects unless asked (`location: true`).
    req.settings.follow_redirects = false;

    let resp_at = lines.iter().position(|l| is_hurl_marker(l) && l.starts_with("HTTP")).unwrap_or(lines.len());
    let (request, response) = lines.split_at(resp_at);
    let mut section = "";
    let mut body_lines: Vec<&str> = Vec::new();
    let mut cookies: Vec<String> = Vec::new();
    let mut form: Vec<KeyValue> = Vec::new();
    let mut parts: Vec<MultipartPart> = Vec::new();
    for l in request.iter().skip(1) {
        let t = l.trim();
        if !body_lines.is_empty() {
            body_lines.push(l);
            continue;
        }
        if t.is_empty() {
            continue;
        }
        if SECTIONS.contains(&t) {
            section = t;
            continue;
        }
        let is_body_start = t.starts_with(['{', '[', '<', '`']) || ["base64,", "file,", "hex,", "json ", "xml "].iter().any(|p| t.starts_with(p));
        if is_body_start {
            body_lines.push(l);
            continue;
        }
        let Some((k, v)) = key_value(t) else { continue };
        match section {
            "" => req.headers.push(KeyValue::new(k, v)),
            "[QueryStringParams]" | "[Query]" => req.params.push(KeyValue::new(k, v)),
            "[FormParams]" | "[Form]" => form.push(KeyValue::new(k, v)),
            "[MultipartFormData]" | "[Multipart]" => {
                let raw = t.split_once(':').map(|x| x.1.trim()).unwrap_or("");
                match raw.strip_prefix("file,") {
                    Some(spec) => {
                        let (path, ct) = spec.split_once(';').unwrap_or((spec, ""));
                        parts.push(MultipartPart {
                            name: k,
                            value: String::new(),
                            file: Some(path.trim().to_string()),
                            content_type: Some(ct.trim().to_string()).filter(|c| !c.is_empty()),
                            enabled: true,
                        });
                    }
                    None => parts.push(MultipartPart { name: k, value: v, file: None, content_type: None, enabled: true }),
                }
            }
            "[Cookies]" => cookies.push(format!("{k}={v}")),
            "[BasicAuth]" => req.auth = Auth::Basic { username: k, password: v },
            "[Options]" => match k.as_str() {
                "insecure" | "k" => req.settings.verify_tls = v != "true",
                "location" | "L" => req.settings.follow_redirects = v == "true",
                "proxy" | "x" => req.settings.proxy = Some(v),
                _ => w.about(&format!("Hurl option “{k}” has no equivalent and was ignored"), &req.name),
            },
            _ => {}
        }
    }
    if !cookies.is_empty() {
        req.headers.push(KeyValue::new("Cookie", cookies.join("; ")));
    }
    if !form.is_empty() {
        req.body = Body::Form { fields: form };
    } else if !parts.is_empty() {
        req.body = Body::Multipart { parts };
    } else if !body_lines.is_empty() {
        let subject = req.name.clone();
        req.body = body(&body_lines, req.headers.iter().find(|h| h.name.eq_ignore_ascii_case("content-type")).map(|h| h.value.as_str()), w, &subject);
    }

    // Response: status, implicit header checks, [Asserts], [Captures].
    let mut kept: Vec<String> = Vec::new();
    let mut section = "";
    for (i, l) in response.iter().enumerate() {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if i == 0 {
            let code = t.rsplit(' ').next().unwrap_or("");
            if code != "*" {
                req.tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some(code.to_string()), enabled: true });
            }
            continue;
        }
        if SECTIONS.contains(&t) {
            section = t;
            continue;
        }
        match section {
            "" => match key_value(t) {
                Some((k, v)) if !t.starts_with(['{', '[', '<', '`']) => {
                    req.tests.push(Assertion { target: AssertTarget::Header { name: k }, op: AssertOp::Eq, value: Some(v), enabled: true })
                }
                _ => kept.push(l.to_string()),
            },
            "[Asserts]" => match assertion(t) {
                Some(a) => req.tests.push(a),
                None => kept.push(format!("[Asserts] {t}")),
            },
            "[Captures]" => match capture(t) {
                Some(c) => req.captures.push(c),
                None => kept.push(format!("[Captures] {t}")),
            },
            _ => kept.push(l.to_string()),
        }
    }
    if !kept.is_empty() {
        w.about("Some Hurl asserts/captures have no equivalent; they were kept in the request docs", &req.name);
        req.docs = Some(format!("Not converted from Hurl:\n\n```hurl\n{}\n```", kept.join("\n")));
    }
    req
}

fn body(lines: &[&str], ct: Option<&str>, w: &mut Warnings, subject: &str) -> Body {
    let first = lines[0].trim();
    if let Some(lang) = first.strip_prefix("```") {
        let end = lines.iter().skip(1).position(|l| l.trim() == "```").map(|p| p + 1).unwrap_or(lines.len());
        let content = lines[1..end].join("\n");
        return match lang.trim() {
            "json" => Body::Json { content },
            "xml" => Body::Xml { content },
            "graphql" => match content.find("variables {") {
                Some(i) => Body::Graphql { query: content[..i].trim().to_string(), variables: content[i + "variables".len()..].trim().to_string(), operation_name: None },
                None => Body::Graphql { query: content, variables: String::new(), operation_name: None },
            },
            _ => body_from_text(content, ct),
        };
    }
    if first.starts_with('`') && first.ends_with('`') && first.len() >= 2 {
        return body_from_text(first[1..first.len() - 1].to_string(), ct);
    }
    if let Some(path) = first.strip_prefix("file,") {
        return Body::Binary { path: path.trim_end_matches(';').trim().to_string(), content_type: ct.map(str::to_string) };
    }
    if first.starts_with("base64,") || first.starts_with("hex,") {
        w.about("Inline base64/hex bodies were not imported", subject);
        return Body::None;
    }
    let text = lines.join("\n").trim().to_string();
    body_from_text(text, ct)
}

/// `query [args] predicate [value]` → assertion, when the query and predicate map 1:1.
fn assertion(t: &str) -> Option<Assertion> {
    let (target, rest) = query(t)?;
    let rest = rest.trim();
    let (negate, rest) = match rest.strip_prefix("not ") {
        Some(r) => (true, r.trim()),
        None => (false, rest),
    };
    let (pred, value) = rest.split_once(char::is_whitespace).map(|(p, v)| (p, Some(v.trim()))).unwrap_or((rest, None));
    let op = match (pred, negate) {
        ("==", false) | ("!=", true) => AssertOp::Eq,
        ("!=", false) | ("==", true) => AssertOp::Ne,
        (">", false) => AssertOp::Gt,
        (">=", false) => AssertOp::Gte,
        ("<", false) => AssertOp::Lt,
        ("<=", false) => AssertOp::Lte,
        ("contains" | "includes", false) => AssertOp::Contains,
        ("contains" | "includes", true) => AssertOp::NotContains,
        ("startsWith", false) => AssertOp::StartsWith,
        ("endsWith", false) => AssertOp::EndsWith,
        ("matches", false) => AssertOp::Matches,
        ("exists", false) => AssertOp::Exists,
        ("exists", true) => AssertOp::NotExists,
        ("isEmpty", false) => AssertOp::IsEmpty,
        ("isEmpty", true) => AssertOp::IsNotEmpty,
        ("isString" | "isNumber" | "isBoolean" | "isCollection" | "isInteger" | "isFloat", false) => {
            let ty = match pred {
                "isString" => "string",
                "isBoolean" => "boolean",
                "isCollection" => "array",
                _ => "number",
            };
            return Some(Assertion { target, op: AssertOp::IsType, value: Some(ty.into()), enabled: true });
        }
        _ => return None,
    };
    let value = value.map(|v| {
        let v = v.trim();
        if v.len() >= 2 && v.starts_with('/') && v.ends_with('/') { v[1..v.len() - 1].to_string() } else { unquote(v) }
    });
    Some(Assertion { target, op, value, enabled: true })
}

fn capture(t: &str) -> Option<Capture> {
    let (name, q) = t.split_once(':')?;
    let q = q.trim();
    let (source, rest) = if q == "status" {
        (CaptureSource::Status, "")
    } else if q == "body" {
        (CaptureSource::Body, "")
    } else if let Some(r) = q.strip_prefix("header ") {
        let (arg, rest) = quoted_arg(r)?;
        (CaptureSource::Header { header: arg }, rest)
    } else if let Some(r) = q.strip_prefix("jsonpath ") {
        let (arg, rest) = quoted_arg(r)?;
        (CaptureSource::JsonPath { path: arg }, rest)
    } else if let Some(r) = q.strip_prefix("cookie ") {
        let (arg, rest) = quoted_arg(r)?;
        (CaptureSource::Cookie { cookie: arg }, rest)
    } else {
        let (arg, rest) = quoted_arg(q.strip_prefix("regex ")?)?;
        (CaptureSource::Regex { pattern: arg, group: 1 }, rest)
    };
    // Filters (`| count`, `nth 0`, …) have no equivalent.
    rest.trim().is_empty().then(|| Capture { name: name.trim().to_string(), source, enabled: true })
}

/// Parses the query part of an assert line; returns the target and the remaining text.
fn query(t: &str) -> Option<(AssertTarget, &str)> {
    if let Some(r) = t.strip_prefix("status") {
        return Some((AssertTarget::Status, r));
    }
    if let Some(r) = t.strip_prefix("duration") {
        return Some((AssertTarget::ResponseTime, r));
    }
    if let Some(r) = t.strip_prefix("bytes count") {
        return Some((AssertTarget::Size, r));
    }
    if let Some(r) = t.strip_prefix("body") {
        return Some((AssertTarget::Body, r));
    }
    if let Some(r) = t.strip_prefix("header ") {
        let (arg, rest) = quoted_arg(r)?;
        return Some((if arg.eq_ignore_ascii_case("content-type") { AssertTarget::ContentType } else { AssertTarget::Header { name: arg } }, rest));
    }
    if let Some(r) = t.strip_prefix("jsonpath ") {
        let (arg, rest) = quoted_arg(r)?;
        // A filter between the query and the predicate (`count`, `nth 0`, …) isn't supported.
        let first = rest.split_whitespace().next().unwrap_or("");
        if first.chars().next().is_some_and(char::is_alphabetic) && !is_predicate(first) && first != "not" {
            return None;
        }
        return Some((AssertTarget::JsonPath { path: arg }, rest));
    }
    None
}

fn is_predicate(p: &str) -> bool {
    matches!(p, "contains" | "includes" | "startsWith" | "endsWith" | "matches" | "exists" | "isEmpty" | "isString" | "isNumber" | "isBoolean" | "isCollection" | "isInteger" | "isFloat")
}

fn quoted_arg(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"')?;
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some((unquote(&s[..i + 2]), &rest[i + 1..])),
            _ => escaped = false,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_asserts_and_captures() {
        let text = "# Login\nPOST https://example.org/api/login\nContent-Type: application/json\n```json\n{\"user\": \"bob\"}\n```\nHTTP 200\nContent-Type: application/json\n[Captures]\ntoken: jsonpath \"$.token\"\ncount: jsonpath \"$.items\" count\n[Asserts]\njsonpath \"$.status\" == \"RUNNING\"\njsonpath \"$.items\" count == 2\nheader \"X-Id\" exists\nduration < 1000\n\nGET https://example.org/api/me?x=1\nAuthorization: Bearer {{token}}\n[QueryStringParams]\npage: 2\n[Options]\nlocation: true\nHTTP *\n";
        let plan = parse(text, "t");
        let reqs = plan.requests();
        assert_eq!(reqs.len(), 2);
        let login = reqs[0].1;
        assert!(matches!(&login.body, Body::Json { content } if content.contains("bob")));
        assert_eq!(login.tests[0], Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("200".into()), enabled: true });
        assert!(login.tests.iter().any(|a| a.target == AssertTarget::JsonPath { path: "$.status".into() } && a.value.as_deref() == Some("RUNNING")));
        assert!(login.tests.iter().any(|a| a.target == AssertTarget::Header { name: "X-Id".into() } && a.op == AssertOp::Exists));
        assert!(login.tests.iter().any(|a| a.target == AssertTarget::ResponseTime && a.op == AssertOp::Lt));
        assert_eq!(login.captures, vec![Capture { name: "token".into(), source: CaptureSource::JsonPath { path: "$.token".into() }, enabled: true }]);
        let docs = login.docs.as_deref().unwrap();
        assert!(docs.contains("count == 2") && docs.contains("count: jsonpath"), "{docs}");
        let me = reqs[1].1;
        assert_eq!(me.params.len(), 2);
        assert!(me.settings.follow_redirects);
        assert!(me.tests.is_empty());
    }
}
