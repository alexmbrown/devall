//! `.http` / `.rest` files: JetBrains HTTP Client and VS Code REST Client dialects. When read
//! from disk, sibling `http-client.env.json` / `http-client.private.env.json` become environments
//! (private values are secrets).

use std::path::Path;

use indexmap::IndexMap;
use serde_json::Value;

use devall_core::{Body, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue, MultipartPart};

use crate::detect::Format;
use crate::plan::{ImportPlan, PlanItem};
use crate::util::{Warnings, body_from_text, default_name, header_value, looks_like_form, map_templates, parse_form_pairs, scalar_string, split_query};

const KNOWN_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT", "QUERY", "GRAPHQL"];

pub(crate) fn parse(text: &str, name: &str) -> ImportPlan {
    let mut plan = ImportPlan::new(name, Format::HttpFile);
    let mut w = Warnings::default();
    let mut file_vars: IndexMap<String, String> = IndexMap::new();
    for (title, lines) in blocks(text) {
        if let Some(req) = block(title, &lines, &mut file_vars, &mut w) {
            plan.items.push(PlanItem::Request(Box::new(req)));
        }
    }
    if !file_vars.is_empty() {
        let vars = file_vars.into_iter().map(|(k, v)| (k, convert_vars(&v, &mut w, ""))).collect();
        plan.settings = Some(FolderFile { variables: vars, ..Default::default() });
    }
    plan.warnings = w.finish();
    plan
}

/// Parses a file on disk, picking up JetBrains environment files next to it.
pub(crate) fn parse_file(text: &str, path: &Path) -> ImportPlan {
    let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "HTTP requests".into());
    let mut plan = parse(text, &name);
    let Some(dir) = path.parent() else { return plan };
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let public = read("http-client.env.json").or_else(|| read("rest-client.env.json"));
    let private = read("http-client.private.env.json");
    if public.is_none() && private.is_none() {
        return plan;
    }
    let mut envs: IndexMap<String, EnvironmentFile> = IndexMap::new();
    let mut shared: IndexMap<String, String> = IndexMap::new();
    for (doc, secret) in [(public, false), (private, true)] {
        let Some(Value::Object(map)) = doc else { continue };
        for (env_name, vars) in map {
            let Value::Object(vars) = vars else { continue };
            for (k, v) in vars {
                if v.is_object() || v.is_array() {
                    continue; // SSL configuration and similar settings
                }
                let value = scalar_string(&v);
                if env_name == "$shared" {
                    shared.insert(k, value);
                    continue;
                }
                let env = envs.entry(env_name.clone()).or_insert_with(|| EnvironmentFile::new(env_name.clone()));
                if secret && !env.secrets.contains(&k) {
                    env.secrets.push(k.clone());
                }
                env.variables.insert(k, value);
            }
        }
    }
    if !shared.is_empty() {
        plan.settings.get_or_insert_with(FolderFile::default).variables.extend(shared);
    }
    plan.environments = envs.into_values().collect();
    plan
}

/// Splits on `###` lines; the text after `###` is the request title.
fn blocks(text: &str) -> Vec<(Option<String>, Vec<&str>)> {
    let mut out: Vec<(Option<String>, Vec<&str>)> = vec![(None, Vec::new())];
    for line in text.trim_start_matches('\u{feff}').lines() {
        if let Some(rest) = line.strip_prefix("###") {
            let title = rest.trim_start_matches('#').trim();
            out.push((Some(title.to_string()).filter(|t| !t.is_empty()), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(line);
        }
    }
    out
}

fn block(title: Option<String>, lines: &[&str], file_vars: &mut IndexMap<String, String>, w: &mut Warnings) -> Option<HttpRequestFile> {
    let mut name = title;
    let mut docs: Vec<String> = Vec::new();
    let mut follow = true;
    let mut timeout_ms: Option<u64> = None;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim();
        if l.is_empty() {
            i += 1;
            continue;
        }
        if l.starts_with('#') || l.starts_with("//") {
            let c = l.trim_start_matches('#').trim_start_matches("//").trim();
            if let Some(n) = c.strip_prefix("@name") {
                let n = n.trim_start_matches([' ', '=']).trim();
                if !n.is_empty() {
                    name = Some(n.to_string());
                }
            } else if c.starts_with("@no-redirect") {
                follow = false;
            } else if let Some(t) = c.strip_prefix("@timeout") {
                let t = t.trim();
                let (num, unit): (String, String) = (t.chars().take_while(char::is_ascii_digit).collect(), t.chars().skip_while(char::is_ascii_digit).collect());
                if let Ok(n) = num.parse::<u64>() {
                    timeout_ms = Some(match unit.trim() {
                        "ms" => n,
                        "m" => n * 60_000,
                        _ => n * 1000,
                    });
                }
            } else if c.starts_with('@') {
                // Other directives (@no-log, @no-cookie-jar, @prompt, …) have no equivalent.
            } else if !c.is_empty() {
                docs.push(c.to_string());
            }
            i += 1;
            continue;
        }
        if l.starts_with('@')
            && let Some((k, v)) = l[1..].split_once('=')
        {
            file_vars.insert(k.trim().to_string(), v.trim().to_string());
            i += 1;
            continue;
        }
        break;
    }
    if i >= lines.len() {
        return None;
    }
    // Request line: `[METHOD] URL [HTTP/x]`.
    let line = lines[i].trim();
    let (mut method, mut url) = match line.split_once(char::is_whitespace) {
        Some((m, rest)) if KNOWN_METHODS.contains(&m) || (m.len() > 2 && m.chars().all(|c| c.is_ascii_uppercase())) => (m.to_string(), rest.trim().to_string()),
        _ => ("GET".to_string(), line.to_string()),
    };
    if let Some(idx) = url.rfind(" HTTP/") {
        url.truncate(idx);
    }
    i += 1;
    while i < lines.len() && lines[i].starts_with([' ', '\t']) && lines[i].trim_start().starts_with(['?', '&']) {
        url.push_str(lines[i].trim());
        i += 1;
    }
    let mut headers: Vec<KeyValue> = Vec::new();
    while i < lines.len() && !lines[i].trim().is_empty() {
        let l = lines[i].trim();
        if !(l.starts_with('#') || l.starts_with("//"))
            && let Some((k, v)) = l.split_once(':')
        {
            headers.push(KeyValue::new(k.trim(), v.trim()));
        }
        i += 1;
    }
    // Body, minus response handlers (`> {% … %}`, `> script.js`) and response refs (`<> file`).
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_handler = false;
    let mut handlers = 0;
    for l in lines.iter().skip(i + 1) {
        if in_handler {
            if l.contains("%}") {
                in_handler = false;
            }
            continue;
        }
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("> {%") {
            handlers += 1;
            in_handler = !rest.contains("%}");
            continue;
        }
        if (t.starts_with("> ") && t.trim_end().ends_with(".js")) || t.starts_with("<> ") {
            handlers += usize::from(t.starts_with("> "));
            continue;
        }
        body_lines.push(l);
    }
    while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
        body_lines.pop();
    }
    let body_text = body_lines.join("\n");

    let subject = name.clone().unwrap_or_else(|| url.clone());
    url = convert_vars(&url, w, &subject);
    for h in &mut headers {
        h.value = convert_vars(&h.value, w, &subject);
    }
    let body_text = convert_vars(&body_text, w, &subject);

    let graphql_header = headers.iter().position(|h| h.name.eq_ignore_ascii_case("x-request-type") && h.value.eq_ignore_ascii_case("graphql"));
    let is_graphql = method == "GRAPHQL" || graphql_header.is_some();
    if let Some(p) = graphql_header {
        headers.remove(p);
    }
    if method == "GRAPHQL" {
        method = "POST".into();
    }
    // `Content-Type: {{contentType}}` is common: resolve file variables before sniffing.
    let ct_raw: Option<String> = header_value(&headers, "content-type")
        .map(|c| map_templates(c, |k| file_vars.get(k).cloned()))
        .filter(|c| !c.contains("{{"));
    let ct = ct_raw.as_deref().map(str::to_ascii_lowercase);
    let trimmed = body_text.trim();
    let body = if trimmed.is_empty() {
        Body::None
    } else if is_graphql {
        graphql(trimmed)
    } else if let Some(path) = trimmed.strip_prefix("< ").filter(|p| !p.contains('\n')) {
        Body::Binary { path: path.trim().to_string(), content_type: header_value(&headers, "content-type").map(str::to_string) }
    } else if ct.as_deref().is_some_and(|c| c.starts_with("multipart/")) {
        let raw_ct = ct_raw.clone().unwrap_or_default();
        let parts = multipart(&body_text, &raw_ct);
        headers.retain(|h| !h.name.eq_ignore_ascii_case("content-type"));
        Body::Multipart { parts }
    } else if ct.as_deref().is_some_and(|c| c.contains("x-www-form-urlencoded")) {
        let joined: String = body_text.lines().map(str::trim).collect();
        if looks_like_form(&joined) { Body::Form { fields: parse_form_pairs(&joined) } } else { Body::Text { content: body_text, content_type: ct.clone() } }
    } else {
        body_from_text(body_text, ct_raw.as_deref())
    };

    let (base, params) = split_query(&url);
    let mut req = HttpRequestFile::new(name.clone().unwrap_or_else(|| default_name(&method, &base)));
    req.method = method;
    req.url = base;
    req.params = params;
    req.headers = headers;
    req.body = body;
    req.settings.follow_redirects = follow;
    if let Some(t) = timeout_ms {
        req.settings.timeout_ms = t;
    }
    if handlers > 0 {
        w.about("Response handler scripts are not run by /dev/all and were dropped", &req.name);
    }
    if !docs.is_empty() {
        req.docs = Some(docs.join("\n"));
    }
    Some(req)
}

fn graphql(text: &str) -> Body {
    // Query, then optionally a blank line and a JSON variables object.
    if let Some(idx) = text.rfind("\n\n") {
        let (q, vars) = text.split_at(idx);
        let vars = vars.trim();
        if vars.starts_with('{') && serde_json::from_str::<Value>(vars).is_ok() {
            return Body::Graphql { query: q.trim().to_string(), variables: vars.to_string(), operation_name: None };
        }
    }
    Body::Graphql { query: text.to_string(), variables: String::new(), operation_name: None }
}

fn multipart(text: &str, ct: &str) -> Vec<MultipartPart> {
    let boundary = ct
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_string())
        .unwrap_or_default();
    if boundary.is_empty() {
        return Vec::new();
    }
    let delim = format!("--{boundary}");
    let mut parts = Vec::new();
    for chunk in text.split(&delim).skip(1) {
        if chunk.starts_with("--") {
            break;
        }
        let chunk = chunk.trim_start_matches(['\r', '\n']);
        let (head, content) = chunk.split_once("\n\n").or_else(|| chunk.split_once("\r\n\r\n")).unwrap_or((chunk, ""));
        let mut part = MultipartPart { name: String::new(), value: String::new(), file: None, content_type: None, enabled: true };
        for h in head.lines() {
            let Some((k, v)) = h.split_once(':') else { continue };
            if k.trim().eq_ignore_ascii_case("content-disposition") {
                for attr in v.split(';') {
                    if let Some(n) = attr.trim().strip_prefix("name=") {
                        part.name = n.trim_matches('"').to_string();
                    }
                }
            } else if k.trim().eq_ignore_ascii_case("content-type") {
                part.content_type = Some(v.trim().to_string());
            }
        }
        let content = content.trim_end_matches(['\r', '\n']);
        match content.trim().strip_prefix("< ") {
            Some(path) if !path.contains('\n') => part.file = Some(path.trim().to_string()),
            _ => part.value = content.to_string(),
        }
        parts.push(part);
    }
    parts
}

/// REST Client / JetBrains dynamic variables → /dev/all equivalents.
pub(crate) fn convert_vars(s: &str, w: &mut Warnings, subject: &str) -> String {
    map_templates(s, |k| {
        let mut words = k.split_whitespace();
        let head = words.next().unwrap_or("");
        let arg = words.next().unwrap_or("").trim_start_matches('%');
        let rep = match head {
            "$guid" | "$uuid" | "$random.uuid" => "{{$uuid}}".to_string(),
            "$timestamp" => "{{$timestamp}}".to_string(),
            "$isoTimestamp" | "$datetime" | "$localDatetime" => "{{$isoTimestamp}}".to_string(),
            "$randomInt" => "{{$randomInt}}".to_string(),
            h if h.starts_with("$random.integer") => "{{$randomInt}}".to_string(),
            "$processEnv" if !arg.is_empty() => format!("{{{{$env.{arg}}}}}"),
            "$dotenv" if !arg.is_empty() => format!("{{{{secret.{arg}}}}}"),
            h if h.contains(".response.") || h.contains(".request.") => {
                if !subject.is_empty() {
                    w.about("Request variables (values from other responses) are not supported; use captures instead", subject);
                }
                return None;
            }
            _ if k.contains(char::is_whitespace) => {
                if !subject.is_empty() {
                    w.about(&format!("Dynamic variable “{head}” has no equivalent and was kept as text"), subject);
                }
                return None;
            }
            _ => return None,
        };
        Some(rep)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_vars_and_bodies() {
        let text = "@host = api.example.com\n@token = abc\n\n### List users\n# Lists every user\nGET https://{{host}}/users?page=1\n    &limit=10\nAuthorization: Bearer {{token}}\n\n###\n# @name createUser\nPOST https://{{host}}/users HTTP/1.1\nContent-Type: application/json\n\n{\n  \"name\": \"{{$guid}}\"\n}\n\n> {%\n  client.global.set(\"id\", response.body.id);\n%}\n\n### Upload\nPOST https://{{host}}/upload\nContent-Type: multipart/form-data; boundary=WebAppBoundary\n\n--WebAppBoundary\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\nContent-Type: image/png\n\n< ./a.png\n--WebAppBoundary\nContent-Disposition: form-data; name=\"note\"\n\nhello\n--WebAppBoundary--\n";
        let plan = parse(text, "api");
        let reqs = plan.requests();
        assert_eq!(reqs.len(), 3);
        let list = reqs[0].1;
        assert_eq!(list.name, "List users");
        assert_eq!(list.url, "https://{{host}}/users");
        assert_eq!(list.params.len(), 2);
        assert_eq!(list.docs.as_deref(), Some("Lists every user"));
        let create = reqs[1].1;
        assert_eq!(create.name, "createUser");
        assert!(matches!(&create.body, Body::Json { content } if content.contains("{{$uuid}}") && !content.contains("client.global")));
        let up = reqs[2].1;
        let Body::Multipart { parts } = &up.body else { panic!("{:?}", up.body) };
        assert_eq!(parts[0].file.as_deref(), Some("./a.png"));
        assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));
        assert_eq!(parts[1].value, "hello");
        assert!(up.headers.is_empty(), "boundary header dropped");
        let vars = &plan.settings.as_ref().unwrap().variables;
        assert_eq!(vars.get("host").map(String::as_str), Some("api.example.com"));
        assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
    }

    #[test]
    fn graphql_and_directives() {
        let text = "### Q\n# @no-redirect\n# @timeout 5\nGRAPHQL https://x.io/graphql\n\nquery { me { id } }\n\n{\"a\": 1}\n";
        let plan = parse(text, "g");
        let r = plan.requests()[0].1;
        assert_eq!(r.method, "POST");
        assert!(!r.settings.follow_redirects);
        assert_eq!(r.settings.timeout_ms, 5000);
        assert!(matches!(&r.body, Body::Graphql { query, variables, .. } if query == "query { me { id } }" && variables == "{\"a\": 1}"));
    }
}
