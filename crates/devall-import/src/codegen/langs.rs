//! One generator per language. Each renders a complete, runnable snippet from a [`Model`]:
//! headers, body (JSON idiomatically where the language allows, raw text otherwise, forms,
//! multipart with files, binary read from a file) and the TLS / redirect / timeout / digest /
//! proxy options, or a comment when the client has no equivalent.

use super::escape::{json_block, json_str, kotlin_str, ps_str, py_value, rust_raw, rust_str, sh_word, single_quoted, swift_str};
use super::{B, Model, seconds};

const BIN_FILE: &str = "body.bin";

fn bin_note(n: usize) -> String {
    format!("{n}-byte binary body: save it as {BIN_FILE}")
}

// ---------- JavaScript ----------

fn js_headers(m: &Model, indent: &str) -> Option<String> {
    if m.headers.is_empty() {
        return None;
    }
    if m.has_duplicate_headers() {
        let rows: Vec<String> = m.headers.iter().map(|(k, v)| format!("{indent}  [{}, {}],", json_str(k), json_str(v))).collect();
        Some(format!("{indent}headers: [\n{}\n{indent}],", rows.join("\n")))
    } else {
        let rows: Vec<String> = m.headers.iter().map(|(k, v)| format!("{indent}  {}: {},", json_str(k), json_str(v))).collect();
        Some(format!("{indent}headers: {{\n{}\n{indent}}},", rows.join("\n")))
    }
}

fn js_form_data(m: &Model) -> Option<String> {
    let B::Multipart(parts) = &m.body else { return None };
    let mut out = String::from("const form = new FormData();\n");
    for p in parts {
        match &p.file {
            Some(f) => {
                let opts = p.content_type.as_ref().map(|c| format!(", {{ type: {} }}", json_str(c))).unwrap_or_default();
                out.push_str(&format!("form.append({}, await fs.openAsBlob({}{opts}), {});\n", json_str(&p.name), json_str(f), json_str(&p.file_name)));
            }
            None => out.push_str(&format!("form.append({}, {});\n", json_str(&p.name), json_str(&p.value))),
        }
    }
    Some(out)
}

fn js_url_params(pairs: &[(String, String)], indent: &str) -> String {
    let rows: Vec<String> = pairs.iter().map(|(k, v)| format!("{indent}  [{}, {}],", json_str(k), json_str(v))).collect();
    format!("new URLSearchParams([\n{}\n{indent}])", rows.join("\n"))
}

pub(crate) fn fetch(m: &Model) -> String {
    let mut out = String::new();
    if m.has_files() {
        out.push_str("import fs from \"node:fs\";\n\n");
    }
    if m.insecure {
        out.push_str("// TLS certificate checks are off for this request; fetch can't disable them\n// (in Node, run with NODE_TLS_REJECT_UNAUTHORIZED=0).\n");
    }
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("// Digest auth (user {}) isn't built into fetch: answer the 401 challenge yourself.\n", json_str(u)));
    }
    if let Some(p) = &m.proxy {
        out.push_str(&format!("// Proxy {p}: fetch uses the environment's proxy settings.\n"));
    }
    if let Some(form) = js_form_data(m) {
        out.push_str(&form);
        out.push('\n');
    }
    let mut opts: Vec<String> = Vec::new();
    if m.method != "GET" || !matches!(m.body, B::None) {
        opts.push(format!("  method: {},", json_str(&m.method)));
    }
    if let Some(h) = js_headers(m, "  ") {
        opts.push(h);
    }
    match &m.body {
        B::None => {}
        B::Text { json: Some(v), .. } => opts.push(format!("  body: JSON.stringify({}),", json_block(v, 2))),
        B::Text { text, .. } => opts.push(format!("  body: {},", json_str(text))),
        B::Binary(n) => opts.push(format!("  body: await fs.openAsBlob({}), // {}", json_str(BIN_FILE), bin_note(*n))),
        B::Form(pairs) => opts.push(format!("  body: {},", js_url_params(pairs, "  "))),
        B::Multipart(_) => opts.push("  body: form,".into()),
    }
    if !m.follow {
        opts.push("  redirect: \"manual\",".into());
    }
    if let Some(t) = m.timeout_ms {
        opts.push(format!("  signal: AbortSignal.timeout({t}),"));
    }
    if opts.is_empty() {
        out.push_str(&format!("const response = await fetch({});\n", json_str(&m.url)));
    } else {
        out.push_str(&format!("const response = await fetch({}, {{\n{}\n}});\n", json_str(&m.url), opts.join("\n")));
    }
    out.push_str("\nconsole.log(response.status);\nconsole.log(await response.text());\n");
    out
}

pub(crate) fn axios(m: &Model) -> String {
    let mut out = String::from("import axios from \"axios\";\n");
    if m.has_files() {
        out.push_str("import fs from \"node:fs\";\n");
    }
    if m.insecure {
        out.push_str("import https from \"node:https\";\n");
    }
    out.push('\n');
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("// Digest auth (user {}) isn't built into axios: answer the 401 challenge yourself.\n", json_str(u)));
    }
    if let Some(form) = js_form_data(m) {
        out.push_str(&form);
        out.push('\n');
    }
    let mut opts: Vec<String> = vec![format!("  method: {},", json_str(&m.method.to_lowercase())), format!("  url: {},", json_str(&m.url))];
    if !m.headers.is_empty() {
        let rows: Vec<String> = m.folded_headers().iter().map(|(k, v)| format!("    {}: {},", json_str(k), json_str(v))).collect();
        opts.push(format!("  headers: {{\n{}\n  }},", rows.join("\n")));
    }
    match &m.body {
        B::None => {}
        B::Text { json: Some(v), .. } => opts.push(format!("  data: {},", json_block(v, 2))),
        B::Text { text, .. } => opts.push(format!("  data: {},", json_str(text))),
        B::Binary(n) => opts.push(format!("  data: fs.readFileSync({}), // {}", json_str(BIN_FILE), bin_note(*n))),
        B::Form(pairs) => opts.push(format!("  data: {},", js_url_params(pairs, "  "))),
        B::Multipart(_) => opts.push("  data: form,".into()),
    }
    if !m.follow {
        opts.push("  maxRedirects: 0,".into());
    }
    if let Some(t) = m.timeout_ms {
        opts.push(format!("  timeout: {t},"));
    }
    if m.insecure {
        opts.push("  httpsAgent: new https.Agent({ rejectUnauthorized: false }),".into());
    }
    if let Some(p) = &m.proxy
        && let Ok(u) = url::Url::parse(p)
    {
        opts.push(format!(
            "  proxy: {{ protocol: {}, host: {}, port: {} }},",
            json_str(u.scheme()),
            json_str(u.host_str().unwrap_or("")),
            u.port_or_known_default().unwrap_or(80)
        ));
    }
    opts.push("  validateStatus: () => true,".into());
    out.push_str(&format!("const response = await axios.request({{\n{}\n}});\n\nconsole.log(response.status);\nconsole.log(response.data);\n", opts.join("\n")));
    out
}

// ---------- Python ----------

fn py_call(func: &str, args: &[String]) -> String {
    let one = format!("{func}({})", args.join(", "));
    if one.len() <= 88 {
        return one;
    }
    format!("{func}(\n{}\n)", args.iter().map(|a| format!("    {a},")).collect::<Vec<_>>().join("\n"))
}

pub(crate) fn python(m: &Model, httpx: bool) -> String {
    let lib = if httpx { "httpx" } else { "requests" };
    let mut out = format!("import {lib}\n");
    if !httpx && m.digest.is_some() {
        out.push_str("from requests.auth import HTTPDigestAuth\n");
    }
    out.push('\n');
    out.push_str(&format!("url = {}\n", json_str(&m.url)));
    let mut args = vec![json_str(&m.method), "url".to_string()];
    if !m.headers.is_empty() {
        let rows: Vec<String> = m.folded_headers().iter().map(|(k, v)| format!("    {}: {},", json_str(k), json_str(v))).collect();
        out.push_str(&format!("headers = {{\n{}\n}}\n", rows.join("\n")));
        args.push("headers=headers".into());
    }
    match &m.body {
        B::None => {}
        B::Text { json: Some(v), .. } => {
            out.push_str(&format!("payload = {}\n", py_value(v, 0)));
            args.push("json=payload".into());
        }
        B::Text { text, .. } => {
            out.push_str(&format!("payload = {}\n", json_str(text)));
            args.push(if httpx { "content=payload.encode(\"utf-8\")" } else { "data=payload.encode(\"utf-8\")" }.into());
        }
        B::Binary(n) => {
            out.push_str(&format!("# {}\n", bin_note(*n)));
            args.push(if httpx { format!("content=open({}, \"rb\").read()", json_str(BIN_FILE)) } else { format!("data=open({}, \"rb\")", json_str(BIN_FILE)) });
        }
        B::Form(pairs) => {
            if httpx {
                // httpx takes a mapping; repeated names become lists.
                let mut map: Vec<(String, Vec<String>)> = Vec::new();
                for (k, v) in pairs {
                    match map.iter_mut().find(|(x, _)| x == k) {
                        Some((_, vals)) => vals.push(v.clone()),
                        None => map.push((k.clone(), vec![v.clone()])),
                    }
                }
                let rows: Vec<String> = map
                    .iter()
                    .map(|(k, vals)| match vals.as_slice() {
                        [one] => format!("    {}: {},", json_str(k), json_str(one)),
                        many => format!("    {}: [{}],", json_str(k), many.iter().map(|v| json_str(v)).collect::<Vec<_>>().join(", ")),
                    })
                    .collect();
                out.push_str(&format!("payload = {{\n{}\n}}\n", rows.join("\n")));
            } else {
                let rows: Vec<String> = pairs.iter().map(|(k, v)| format!("    ({}, {}),", json_str(k), json_str(v))).collect();
                out.push_str(&format!("payload = [\n{}\n]\n", rows.join("\n")));
            }
            args.push("data=payload".into());
        }
        B::Multipart(parts) => {
            let rows: Vec<String> = parts
                .iter()
                .map(|p| match &p.file {
                    Some(f) => {
                        let ct = p.content_type.as_ref().map(|c| format!(", {}", json_str(c))).unwrap_or_default();
                        format!("    ({}, ({}, open({}, \"rb\"){ct})),", json_str(&p.name), json_str(&p.file_name), json_str(f))
                    }
                    None => format!("    ({}, (None, {})),", json_str(&p.name), json_str(&p.value)),
                })
                .collect();
            out.push_str(&format!("files = [\n{}\n]\n", rows.join("\n")));
            args.push("files=files".into());
        }
    }
    if m.insecure {
        args.push("verify=False".into());
    }
    if httpx && m.follow {
        args.push("follow_redirects=True".into());
    }
    if !httpx && !m.follow {
        args.push("allow_redirects=False".into());
    }
    if let Some(t) = m.timeout_ms {
        args.push(format!("timeout={}", seconds(t)));
    }
    if let Some((u, p)) = &m.digest {
        args.push(if httpx { format!("auth=httpx.DigestAuth({}, {})", json_str(u), json_str(p)) } else { format!("auth=HTTPDigestAuth({}, {})", json_str(u), json_str(p)) });
    }
    if let Some(p) = &m.proxy {
        args.push(if httpx { format!("proxy={}", json_str(p)) } else { format!("proxies={{\"http\": {0}, \"https\": {0}}}", json_str(p)) });
    }
    out.push_str(&format!("\nresponse = {}\n\nprint(response.status_code)\nprint(response.text)\n", py_call(&format!("{lib}.request"), &args)));
    out
}

// ---------- Go ----------

fn go_lit(s: &str) -> String {
    if s.contains('\n') && !s.contains('`') && !s.contains('\r') {
        format!("`{s}`")
    } else {
        json_str(s)
    }
}

pub(crate) fn go(m: &Model) -> String {
    let mut imports: Vec<&str> = vec!["fmt", "io", "net/http"];
    let mut body_setup = String::new();
    let mut body_arg = "nil".to_string();
    let mut after_req = String::new();
    match &m.body {
        B::None => {}
        B::Text { text, .. } => {
            imports.push("strings");
            body_setup = format!("\tbody := strings.NewReader({})\n\n", go_lit(text));
            body_arg = "body".into();
        }
        B::Binary(n) => {
            imports.push("os");
            body_setup = format!("\t// {}\n\tbody, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer body.Close()\n\n", bin_note(*n), json_str(BIN_FILE));
            body_arg = "body".into();
        }
        B::Form(pairs) => {
            imports.push("net/url");
            imports.push("strings");
            body_setup.push_str("\tform := url.Values{}\n");
            for (k, v) in pairs {
                body_setup.push_str(&format!("\tform.Add({}, {})\n", json_str(k), json_str(v)));
            }
            body_setup.push_str("\tbody := strings.NewReader(form.Encode())\n\n");
            body_arg = "body".into();
        }
        B::Multipart(parts) => {
            imports.push("bytes");
            imports.push("mime/multipart");
            body_setup.push_str("\tvar body bytes.Buffer\n\twriter := multipart.NewWriter(&body)\n");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        imports.push("os");
                        if let Some(ct) = &p.content_type {
                            body_setup.push_str(&format!("\t// {} is sent as application/octet-stream; use writer.CreatePart for {ct}\n", json_str(&p.name)));
                        }
                        body_setup.push_str(&format!(
                            "\t{{\n\t\tfile, err := os.Open({})\n\t\tif err != nil {{\n\t\t\tpanic(err)\n\t\t}}\n\t\tpart, err := writer.CreateFormFile({}, {})\n\t\tif err != nil {{\n\t\t\tpanic(err)\n\t\t}}\n\t\tif _, err := io.Copy(part, file); err != nil {{\n\t\t\tpanic(err)\n\t\t}}\n\t\tfile.Close()\n\t}}\n",
                            json_str(f),
                            json_str(&p.name),
                            json_str(&p.file_name)
                        ));
                    }
                    None => body_setup.push_str(&format!("\tif err := writer.WriteField({}, {}); err != nil {{\n\t\tpanic(err)\n\t}}\n", json_str(&p.name), json_str(&p.value))),
                }
            }
            body_setup.push_str("\tif err := writer.Close(); err != nil {\n\t\tpanic(err)\n\t}\n\n");
            body_arg = "&body".into();
            after_req = "\treq.Header.Set(\"Content-Type\", writer.FormDataContentType())\n".into();
        }
    }
    let custom_client = m.insecure || !m.follow || m.timeout_ms.is_some() || m.proxy.is_some();
    let mut fields: Vec<String> = Vec::new();
    if let Some(t) = m.timeout_ms {
        imports.push("time");
        fields.push(format!("\t\tTimeout: {t} * time.Millisecond,"));
    }
    if !m.follow {
        fields.push("\t\tCheckRedirect: func(req *http.Request, via []*http.Request) error {\n\t\t\treturn http.ErrUseLastResponse\n\t\t},".into());
    }
    if m.insecure || m.proxy.is_some() {
        let mut t = Vec::new();
        if m.insecure {
            imports.push("crypto/tls");
            t.push("\t\t\tTLSClientConfig: &tls.Config{InsecureSkipVerify: true},".to_string());
        }
        if let Some(p) = &m.proxy {
            imports.push("net/url");
            t.push(format!("\t\t\tProxy: func(*http.Request) (*url.URL, error) {{ return url.Parse({}) }},", json_str(p)));
        }
        fields.push(format!("\t\tTransport: &http.Transport{{\n{}\n\t\t}},", t.join("\n")));
    }
    imports.sort();
    imports.dedup();

    let mut out = String::from("package main\n\nimport (\n");
    for i in &imports {
        out.push_str(&format!("\t\"{i}\"\n"));
    }
    out.push_str(")\n\nfunc main() {\n");
    out.push_str(&body_setup);
    out.push_str(&format!("\treq, err := http.NewRequest({}, {}, {body_arg})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n", json_str(&m.method), json_str(&m.url)));
    for (k, v) in &m.headers {
        out.push_str(&format!("\treq.Header.Add({}, {})\n", json_str(k), json_str(v)));
    }
    out.push_str(&after_req);
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("\t// Digest auth (user {}): net/http has no built-in digest support.\n", json_str(u)));
    }
    out.push('\n');
    if custom_client {
        out.push_str(&format!("\tclient := &http.Client{{\n{}\n\t}}\n", fields.join("\n")));
    } else {
        out.push_str("\tclient := http.DefaultClient\n");
    }
    out.push_str("\tres, err := client.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer res.Body.Close()\n\n\tdata, err := io.ReadAll(res.Body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tfmt.Println(res.Status)\n\tfmt.Println(string(data))\n}\n");
    out
}

// ---------- Rust ----------

pub(crate) fn rust(m: &Model) -> String {
    let mut features: Vec<&str> = Vec::new();
    let mut builder: Vec<String> = Vec::new();
    if m.insecure {
        builder.push(".tls_danger_accept_invalid_certs(true)".into());
    }
    if !m.follow {
        builder.push(".redirect(reqwest::redirect::Policy::none())".into());
    }
    if let Some(t) = m.timeout_ms {
        builder.push(format!(".timeout(std::time::Duration::from_millis({t}))"));
    }
    if let Some(p) = &m.proxy {
        builder.push(format!(".proxy(reqwest::Proxy::all({})?)", rust_str(p)));
    }
    let method = match m.method.as_str() {
        "GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "CONNECT" | "PATCH" | "TRACE" => format!("reqwest::Method::{}", m.method),
        other => format!("reqwest::Method::from_bytes({})?", rust_str(other).replacen('"', "b\"", 1)),
    };
    let mut chain: Vec<String> = vec![format!(".request({method}, {})", rust_str(&m.url))];
    for (k, v) in &m.headers {
        chain.push(format!(".header({}, {})", rust_str(k), rust_str(v)));
    }
    let mut setup = String::new();
    match &m.body {
        B::None => {}
        B::Text { text, .. } => chain.push(format!(".body({})", rust_raw(text))),
        B::Binary(n) => chain.push(format!(".body(std::fs::read({})?) // {}", rust_str(BIN_FILE), bin_note(*n))),
        B::Form(pairs) => {
            features.push("form");
            chain.push(format!(".form(&[{}])", pairs.iter().map(|(k, v)| format!("({}, {})", rust_str(k), rust_str(v))).collect::<Vec<_>>().join(", ")));
        }
        B::Multipart(parts) => {
            features.push("multipart");
            setup.push_str("    let form = reqwest::multipart::Form::new()");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        let mut part = format!("reqwest::multipart::Part::bytes(std::fs::read({})?).file_name({})", rust_str(f), rust_str(&p.file_name));
                        if let Some(ct) = &p.content_type {
                            part.push_str(&format!(".mime_str({})?", rust_str(ct)));
                        }
                        setup.push_str(&format!("\n        .part({}, {part})", rust_str(&p.name)));
                    }
                    None => setup.push_str(&format!("\n        .text({}, {})", rust_str(&p.name), rust_str(&p.value))),
                }
            }
            setup.push_str(";\n\n");
            chain.push(".multipart(form)".into());
        }
    }
    chain.push(".send()".into());
    chain.push(".await?;".into());
    let feats = if features.is_empty() { String::new() } else { format!(", features = [{}]", features.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ")) };
    let mut out = format!("// Cargo.toml: reqwest = {{ version = \"0.13\"{feats} }}, tokio = {{ version = \"1\", features = [\"full\"] }}\n");
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("// Digest auth (user {}): reqwest has no built-in digest support.\n", rust_str(u)));
    }
    out.push_str("\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {\n");
    if builder.is_empty() {
        out.push_str("    let client = reqwest::Client::new();\n\n");
    } else {
        out.push_str(&format!("    let client = reqwest::Client::builder()\n{}\n        .build()?;\n\n", builder.iter().map(|b| format!("        {b}")).collect::<Vec<_>>().join("\n")));
    }
    out.push_str(&setup);
    out.push_str(&format!("    let response = client\n{}\n\n", chain.iter().map(|c| format!("        {c}")).collect::<Vec<_>>().join("\n")));
    // (Continuation lines of raw-string bodies stay unindented: they are request content.)
    out.push_str("    println!(\"{}\", response.status());\n    println!(\"{}\", response.text().await?);\n    Ok(())\n}\n");
    out
}

// ---------- Java ----------

const JAVA_RESTRICTED: &[&str] = &["connection", "content-length", "expect", "host", "upgrade"];

fn multipart_head(name: &str, file_name: Option<&str>, ct: Option<&str>) -> String {
    let mut h = format!("Content-Disposition: form-data; name=\"{name}\"");
    if let Some(f) = file_name {
        h.push_str(&format!("; filename=\"{f}\""));
    }
    h.push_str("\r\n");
    if let Some(c) = ct {
        h.push_str(&format!("Content-Type: {c}\r\n"));
    }
    h.push_str("\r\n");
    h
}

pub(crate) fn java(m: &Model) -> String {
    let mut imports: Vec<&str> = vec!["java.net.URI", "java.net.http.HttpClient", "java.net.http.HttpRequest", "java.net.http.HttpResponse"];
    let mut setup = String::new();
    let mut headers: Vec<String> = Vec::new();
    let mut multipart_header: Option<String> = None;
    let publisher = match &m.body {
        B::None => "HttpRequest.BodyPublishers.noBody()".to_string(),
        B::Text { text, .. } => format!("HttpRequest.BodyPublishers.ofString({})", json_str(text)),
        B::Binary(n) => {
            imports.push("java.nio.file.Path");
            setup.push_str(&format!("        // {}\n", bin_note(*n)));
            format!("HttpRequest.BodyPublishers.ofFile(Path.of({}))", json_str(BIN_FILE))
        }
        B::Form(pairs) => format!("HttpRequest.BodyPublishers.ofString({})", json_str(&Model::form_encoded(pairs))),
        B::Multipart(parts) => {
            imports.extend(["java.io.ByteArrayOutputStream", "java.nio.charset.StandardCharsets", "java.util.UUID"]);
            setup.push_str("        String boundary = \"----devall\" + UUID.randomUUID();\n        ByteArrayOutputStream body = new ByteArrayOutputStream();\n");
            for p in parts {
                let head = multipart_head(&p.name, p.file.as_ref().map(|_| p.file_name.as_str()), p.content_type.as_deref());
                setup.push_str(&format!("        body.write((\"--\" + boundary + \"\\r\\n\" + {}).getBytes(StandardCharsets.UTF_8));\n", json_str(&head)));
                match &p.file {
                    Some(f) => {
                        imports.extend(["java.nio.file.Files", "java.nio.file.Path"]);
                        setup.push_str(&format!("        body.write(Files.readAllBytes(Path.of({})));\n", json_str(f)));
                    }
                    None => setup.push_str(&format!("        body.write({}.getBytes(StandardCharsets.UTF_8));\n", json_str(&p.value))),
                }
                setup.push_str("        body.write(\"\\r\\n\".getBytes(StandardCharsets.UTF_8));\n");
            }
            setup.push_str("        body.write((\"--\" + boundary + \"--\\r\\n\").getBytes(StandardCharsets.UTF_8));\n\n");
            multipart_header = Some(".header(\"Content-Type\", \"multipart/form-data; boundary=\" + boundary)".to_string());
            "HttpRequest.BodyPublishers.ofByteArray(body.toByteArray())".to_string()
        }
    };
    let mut notes: Vec<String> = Vec::new();
    for (k, v) in &m.headers {
        if JAVA_RESTRICTED.contains(&k.to_ascii_lowercase().as_str()) {
            notes.push(format!("// The {k} header is managed by HttpClient and was left out."));
        } else {
            headers.push(format!(".header({}, {})", json_str(k), json_str(v)));
        }
    }
    headers.extend(multipart_header);
    if m.timeout_ms.is_some() {
        imports.push("java.time.Duration");
    }
    if m.insecure {
        notes.push("// TLS certificate checks are off for this request: build the client with an SSLContext that trusts the server.".into());
    }
    if let Some((u, _)) = &m.digest {
        notes.push(format!("// Digest auth (user {}): HttpClient's Authenticator only answers Basic challenges.", json_str(u)));
    }
    if let Some(p) = &m.proxy {
        notes.push(format!("// Proxy {p}: add .proxy(ProxySelector.of(new InetSocketAddress(host, port))) to the client builder."));
    }
    imports.sort();
    imports.dedup();
    let mut out = String::new();
    for i in &imports {
        out.push_str(&format!("import {i};\n"));
    }
    out.push('\n');
    for n in &notes {
        out.push_str(n);
        out.push('\n');
    }
    out.push_str("public class Main {\n    public static void main(String[] args) throws Exception {\n");
    out.push_str("        HttpClient client = HttpClient.newBuilder()\n");
    if m.follow {
        out.push_str("            .followRedirects(HttpClient.Redirect.NORMAL)\n");
    }
    out.push_str("            .build();\n\n");
    out.push_str(&setup);
    out.push_str(&format!("        HttpRequest request = HttpRequest.newBuilder()\n            .uri(URI.create({}))\n", json_str(&m.url)));
    if let Some(t) = m.timeout_ms {
        out.push_str(&format!("            .timeout(Duration.ofMillis({t}))\n"));
    }
    for h in &headers {
        out.push_str(&format!("            {h}\n"));
    }
    out.push_str(&format!("            .method({}, {publisher})\n            .build();\n\n", json_str(&m.method)));
    out.push_str("        HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());\n        System.out.println(response.statusCode());\n        System.out.println(response.body());\n    }\n}\n");
    out
}

// ---------- Kotlin ----------

pub(crate) fn kotlin(m: &Model) -> String {
    let mut imports: Vec<&str> = vec!["okhttp3.OkHttpClient", "okhttp3.Request"];
    let ct = m.content_type().map(str::to_string);
    let mut skip_ct = false;
    let mut setup = String::new();
    let media = |ct: Option<&str>, imports: &mut Vec<&str>| match ct {
        Some(c) => {
            imports.push("okhttp3.MediaType.Companion.toMediaType");
            format!("{}.toMediaType()", kotlin_str(c))
        }
        None => "null".to_string(),
    };
    let body_expr: Option<String> = match &m.body {
        B::None => None,
        B::Text { text, .. } => {
            imports.push("okhttp3.RequestBody.Companion.toRequestBody");
            skip_ct = ct.is_some();
            Some(format!("{}.toRequestBody({})", kotlin_str(text), media(ct.as_deref(), &mut imports)))
        }
        B::Binary(n) => {
            imports.extend(["java.io.File", "okhttp3.RequestBody.Companion.asRequestBody"]);
            skip_ct = ct.is_some();
            setup.push_str(&format!("// {}\n", bin_note(*n)));
            Some(format!("File({}).asRequestBody({})", kotlin_str(BIN_FILE), media(ct.as_deref(), &mut imports)))
        }
        B::Form(pairs) => {
            imports.push("okhttp3.FormBody");
            skip_ct = true;
            setup.push_str("val formBody = FormBody.Builder()\n");
            for (k, v) in pairs {
                setup.push_str(&format!("    .add({}, {})\n", kotlin_str(k), kotlin_str(v)));
            }
            setup.push_str("    .build()\n\n");
            Some("formBody".into())
        }
        B::Multipart(parts) => {
            imports.push("okhttp3.MultipartBody");
            setup.push_str("val multipartBody = MultipartBody.Builder()\n    .setType(MultipartBody.FORM)\n");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        imports.extend(["java.io.File", "okhttp3.RequestBody.Companion.asRequestBody"]);
                        let mt = media(p.content_type.as_deref(), &mut imports);
                        setup.push_str(&format!("    .addFormDataPart({}, {}, File({}).asRequestBody({mt}))\n", kotlin_str(&p.name), kotlin_str(&p.file_name), kotlin_str(f)));
                    }
                    None => setup.push_str(&format!("    .addFormDataPart({}, {})\n", kotlin_str(&p.name), kotlin_str(&p.value))),
                }
            }
            setup.push_str("    .build()\n\n");
            Some("multipartBody".into())
        }
    };
    let needs_body = matches!(m.method.as_str(), "POST" | "PUT" | "PATCH" | "PROPPATCH" | "REPORT");
    let body_arg = match body_expr {
        Some(b) => b,
        None if needs_body => {
            imports.push("okhttp3.RequestBody.Companion.toRequestBody");
            "ByteArray(0).toRequestBody(null)".into()
        }
        None => "null".into(),
    };
    let mut client: Vec<String> = Vec::new();
    if !m.follow {
        client.push(".followRedirects(false)".into());
    }
    if let Some(t) = m.timeout_ms {
        imports.push("java.util.concurrent.TimeUnit");
        client.push(format!(".callTimeout({t}, TimeUnit.MILLISECONDS)"));
    }
    imports.sort();
    imports.dedup();
    let mut out = String::new();
    for i in &imports {
        out.push_str(&format!("import {i}\n"));
    }
    out.push('\n');
    if m.insecure {
        out.push_str("// TLS certificate checks are off for this request: configure sslSocketFactory/hostnameVerifier to trust the server.\n");
    }
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("// Digest auth (user {}): OkHttp needs an Authenticator (e.g. okhttp-digest).\n", kotlin_str(u)));
    }
    if let Some(p) = &m.proxy {
        out.push_str(&format!("// Proxy {p}: add .proxy(Proxy(Proxy.Type.HTTP, InetSocketAddress(host, port))) to the client builder.\n"));
    }
    if client.is_empty() {
        out.push_str("val client = OkHttpClient()\n\n");
    } else {
        out.push_str(&format!("val client = OkHttpClient.Builder()\n{}\n    .build()\n\n", client.iter().map(|c| format!("    {c}")).collect::<Vec<_>>().join("\n")));
    }
    out.push_str(&setup);
    out.push_str(&format!("val request = Request.Builder()\n    .url({})\n    .method({}, {body_arg})\n", kotlin_str(&m.url), kotlin_str(&m.method)));
    for (k, v) in &m.headers {
        if skip_ct && k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        out.push_str(&format!("    .addHeader({}, {})\n", kotlin_str(k), kotlin_str(v)));
    }
    out.push_str("    .build()\n\nclient.newCall(request).execute().use { response ->\n    println(response.code)\n    println(response.body?.string())\n}\n");
    out
}

// ---------- C# ----------

const CONTENT_HEADERS: &[&str] = &["content-type", "content-encoding", "content-language", "content-disposition", "content-location", "content-md5", "content-range", "expires", "last-modified", "allow"];

pub(crate) fn csharp(m: &Model) -> String {
    let mut usings: Vec<&str> = vec!["System.Net.Http"];
    let mut handler: Vec<String> = Vec::new();
    if !m.follow {
        handler.push("    AllowAutoRedirect = false,".into());
    }
    if m.insecure {
        handler.push("    ServerCertificateCustomValidationCallback = HttpClientHandler.DangerousAcceptAnyServerCertificateValidator,".into());
    }
    if let Some((u, p)) = &m.digest {
        usings.push("System.Net");
        handler.push(format!("    Credentials = new NetworkCredential({}, {}),", json_str(u), json_str(p)));
    }
    if let Some(p) = &m.proxy {
        usings.push("System.Net");
        handler.push(format!("    Proxy = new WebProxy({}),", json_str(p)));
        handler.push("    UseProxy = true,".into());
    }
    let mut body = String::new();
    let has_content = !matches!(m.body, B::None);
    match &m.body {
        B::None => {}
        B::Text { text, .. } => body.push_str(&format!("request.Content = new StringContent({});\n", json_str(text))),
        B::Binary(n) => body.push_str(&format!("// {}\nrequest.Content = new ByteArrayContent(File.ReadAllBytes({}));\n", bin_note(*n), json_str(BIN_FILE))),
        B::Form(pairs) => {
            body.push_str("request.Content = new FormUrlEncodedContent(new[]\n{\n");
            for (k, v) in pairs {
                body.push_str(&format!("    new KeyValuePair<string, string>({}, {}),\n", json_str(k), json_str(v)));
            }
            body.push_str("});\n");
        }
        B::Multipart(parts) => {
            usings.push("System.Net.Http.Headers");
            body.push_str("var form = new MultipartFormDataContent();\n");
            for (i, p) in parts.iter().enumerate() {
                match &p.file {
                    Some(f) => {
                        let var = format!("file{}", i + 1);
                        body.push_str(&format!("var {var} = new ByteArrayContent(File.ReadAllBytes({}));\n", json_str(f)));
                        if let Some(ct) = &p.content_type {
                            body.push_str(&format!("{var}.Headers.ContentType = MediaTypeHeaderValue.Parse({});\n", json_str(ct)));
                        }
                        body.push_str(&format!("form.Add({var}, {}, {});\n", json_str(&p.name), json_str(&p.file_name)));
                    }
                    None => body.push_str(&format!("form.Add(new StringContent({}), {});\n", json_str(&p.value), json_str(&p.name))),
                }
            }
            body.push_str("request.Content = form;\n");
        }
    }
    let mut headers = String::new();
    for (k, v) in &m.headers {
        let l = k.to_ascii_lowercase();
        if CONTENT_HEADERS.contains(&l.as_str()) {
            if !has_content {
                headers.push_str(&format!("// {k} is a content header and this request has no body.\n"));
            } else if l == "content-type" && matches!(m.body, B::Form(_)) {
                // FormUrlEncodedContent sets it.
            } else if l == "content-type" {
                usings.push("System.Net.Http.Headers");
                body.push_str(&format!("request.Content.Headers.ContentType = MediaTypeHeaderValue.Parse({});\n", json_str(v)));
            } else {
                body.push_str(&format!("request.Content.Headers.TryAddWithoutValidation({}, {});\n", json_str(k), json_str(v)));
            }
        } else {
            headers.push_str(&format!("request.Headers.TryAddWithoutValidation({}, {});\n", json_str(k), json_str(v)));
        }
    }
    usings.sort();
    usings.dedup();
    let mut out = String::new();
    for u in &usings {
        out.push_str(&format!("using {u};\n"));
    }
    out.push('\n');
    if handler.is_empty() {
        out.push_str("using var client = new HttpClient();\n");
    } else {
        out.push_str(&format!("var handler = new HttpClientHandler\n{{\n{}\n}};\nusing var client = new HttpClient(handler);\n", handler.join("\n")));
    }
    if let Some(t) = m.timeout_ms {
        out.push_str(&format!("client.Timeout = TimeSpan.FromMilliseconds({t});\n"));
    }
    out.push_str(&format!("\nusing var request = new HttpRequestMessage(new HttpMethod({}), {});\n", json_str(&m.method), json_str(&m.url)));
    out.push_str(&headers);
    out.push_str(&body);
    out.push_str("\nusing var response = await client.SendAsync(request);\nConsole.WriteLine((int)response.StatusCode);\nConsole.WriteLine(await response.Content.ReadAsStringAsync());\n");
    out
}

// ---------- PHP ----------

pub(crate) fn php(m: &Model) -> String {
    let q = single_quoted;
    let mut opts: Vec<String> = vec![format!("CURLOPT_URL => {}", q(&m.url)), "CURLOPT_RETURNTRANSFER => true".into()];
    match m.method.as_str() {
        "GET" if matches!(m.body, B::None) => {}
        "HEAD" => opts.push("CURLOPT_NOBODY => true".into()),
        other => opts.push(format!("CURLOPT_CUSTOMREQUEST => {}", q(other))),
    }
    if !m.headers.is_empty() {
        let rows: Vec<String> = m.headers.iter().map(|(k, v)| format!("        {},", q(&format!("{k}: {v}")))).collect();
        opts.push(format!("CURLOPT_HTTPHEADER => [\n{}\n    ]", rows.join("\n")));
    }
    let mut pre = String::new();
    match &m.body {
        B::None => {}
        B::Text { text, .. } => opts.push(format!("CURLOPT_POSTFIELDS => {}", q(text))),
        B::Binary(n) => {
            pre.push_str(&format!("// {}\n", bin_note(*n)));
            opts.push(format!("CURLOPT_POSTFIELDS => file_get_contents({})", q(BIN_FILE)));
        }
        B::Form(pairs) => opts.push(format!("CURLOPT_POSTFIELDS => {}", q(&Model::form_encoded(pairs)))),
        B::Multipart(parts) => {
            let rows: Vec<String> = parts
                .iter()
                .map(|p| match &p.file {
                    Some(f) => format!("        {} => new CURLFile({}, {}, {}),", q(&p.name), q(f), q(p.content_type.as_deref().unwrap_or("application/octet-stream")), q(&p.file_name)),
                    None => format!("        {} => {},", q(&p.name), q(&p.value)),
                })
                .collect();
            opts.push(format!("CURLOPT_POSTFIELDS => [\n{}\n    ]", rows.join("\n")));
        }
    }
    if m.follow {
        opts.push("CURLOPT_FOLLOWLOCATION => true".into());
    }
    if m.insecure {
        opts.push("CURLOPT_SSL_VERIFYPEER => false".into());
        opts.push("CURLOPT_SSL_VERIFYHOST => 0".into());
    }
    if let Some(t) = m.timeout_ms {
        opts.push(format!("CURLOPT_TIMEOUT_MS => {t}"));
    }
    if let Some((u, p)) = &m.digest {
        opts.push("CURLOPT_HTTPAUTH => CURLAUTH_DIGEST".into());
        opts.push(format!("CURLOPT_USERPWD => {}", q(&format!("{u}:{p}"))));
    }
    if let Some(p) = &m.proxy {
        opts.push(format!("CURLOPT_PROXY => {}", q(p)));
    }
    format!(
        "<?php\n\n{pre}$ch = curl_init();\n\ncurl_setopt_array($ch, [\n{}\n]);\n\n$response = curl_exec($ch);\nif ($response === false) {{\n    echo 'Error: ' . curl_error($ch) . PHP_EOL;\n}} else {{\n    echo curl_getinfo($ch, CURLINFO_RESPONSE_CODE) . PHP_EOL;\n    echo $response . PHP_EOL;\n}}\ncurl_close($ch);\n",
        opts.iter().map(|o| format!("    {o},")).collect::<Vec<_>>().join("\n")
    )
}

// ---------- Ruby ----------

pub(crate) fn ruby(m: &Model) -> String {
    let q = single_quoted;
    let mut out = String::from("require 'net/http'\nrequire 'uri'\n");
    if m.insecure {
        out.push_str("require 'openssl'\n");
    }
    out.push('\n');
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("# Digest auth (user {}): Net::HTTP needs the net-http-digest_auth gem.\n", q(u)));
    }
    out.push_str(&format!("uri = URI({})\n", q(&m.url)));
    match m.proxy.as_deref().and_then(|p| url::Url::parse(p).ok()) {
        Some(p) => out.push_str(&format!("http = Net::HTTP.new(uri.host, uri.port, {}, {})\n", q(p.host_str().unwrap_or("")), p.port_or_known_default().unwrap_or(80))),
        None => out.push_str("http = Net::HTTP.new(uri.host, uri.port)\n"),
    }
    out.push_str("http.use_ssl = uri.scheme == 'https'\n");
    if m.insecure {
        out.push_str("http.verify_mode = OpenSSL::SSL::VERIFY_NONE\n");
    }
    if let Some(t) = m.timeout_ms {
        out.push_str(&format!("http.open_timeout = {0}\nhttp.read_timeout = {0}\n", seconds(t)));
    }
    let class = match m.method.as_str() {
        "GET" => Some("Get"),
        "POST" => Some("Post"),
        "PUT" => Some("Put"),
        "PATCH" => Some("Patch"),
        "DELETE" => Some("Delete"),
        "HEAD" => Some("Head"),
        "OPTIONS" => Some("Options"),
        "TRACE" => Some("Trace"),
        _ => None,
    };
    out.push('\n');
    match class {
        Some(c) => out.push_str(&format!("request = Net::HTTP::{c}.new(uri)\n")),
        None => out.push_str(&format!("request = Net::HTTPGenericRequest.new({}, true, true, uri)\n", q(&m.method))),
    }
    let mut seen: Vec<String> = Vec::new();
    for (k, v) in &m.headers {
        if seen.iter().any(|s| s.eq_ignore_ascii_case(k)) {
            out.push_str(&format!("request.add_field({}, {})\n", q(k), q(v)));
        } else {
            out.push_str(&format!("request[{}] = {}\n", q(k), q(v)));
            seen.push(k.clone());
        }
    }
    match &m.body {
        B::None => {}
        B::Text { text, .. } => out.push_str(&format!("request.body = {}\n", q(text))),
        B::Binary(n) => out.push_str(&format!("request.body = File.binread({}) # {}\n", q(BIN_FILE), bin_note(*n))),
        B::Form(pairs) => out.push_str(&format!("request.body = URI.encode_www_form([{}])\n", pairs.iter().map(|(k, v)| format!("[{}, {}]", q(k), q(v))).collect::<Vec<_>>().join(", "))),
        B::Multipart(parts) => {
            let rows: Vec<String> = parts
                .iter()
                .map(|p| match &p.file {
                    Some(f) => {
                        let ct = p.content_type.as_ref().map(|c| format!(", content_type: {}", q(c))).unwrap_or_default();
                        format!("  [{}, File.open({}), {{ filename: {}{ct} }}]", q(&p.name), q(f), q(&p.file_name))
                    }
                    None => format!("  [{}, {}]", q(&p.name), q(&p.value)),
                })
                .collect();
            out.push_str(&format!("request.set_form([\n{}\n], 'multipart/form-data')\n", rows.join(",\n")));
        }
    }
    out.push_str("\nresponse = http.request(request)\nputs response.code\nputs response.body\n");
    out
}

// ---------- Swift ----------

fn swift_lit(s: &str) -> String {
    if s.contains('\n') && !s.contains('\r') {
        let escaped = s.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
        format!("\"\"\"\n{escaped}\n\"\"\"")
    } else {
        swift_str(s)
    }
}

pub(crate) fn swift(m: &Model) -> String {
    let mut out = String::from("import Foundation\n\n");
    if m.insecure {
        out.push_str("// TLS certificate checks are off for this request: use a URLSessionDelegate that trusts the server.\n");
    }
    if !m.follow {
        out.push_str("// Redirects are not followed for this request: implement urlSession(_:task:willPerformHTTPRedirection:...) and pass nil.\n");
    }
    if let Some((u, _)) = &m.digest {
        out.push_str(&format!("// Digest auth (user {}): answer the challenge in urlSession(_:task:didReceive:completionHandler:).\n", swift_str(u)));
    }
    if let Some(p) = &m.proxy {
        out.push_str(&format!("// Proxy {p}: set connectionProxyDictionary on a URLSessionConfiguration.\n"));
    }
    out.push_str(&format!("var request = URLRequest(url: URL(string: {})!)\nrequest.httpMethod = {}\n", swift_str(&m.url), swift_str(&m.method)));
    if let Some(t) = m.timeout_ms {
        out.push_str(&format!("request.timeoutInterval = {}\n", seconds(t)));
    }
    for (k, v) in &m.headers {
        out.push_str(&format!("request.addValue({}, forHTTPHeaderField: {})\n", swift_str(v), swift_str(k)));
    }
    match &m.body {
        B::None => {}
        B::Text { text, .. } => out.push_str(&format!("request.httpBody = Data({}.utf8)\n", swift_lit(text))),
        B::Binary(n) => out.push_str(&format!("// {}\nrequest.httpBody = try Data(contentsOf: URL(fileURLWithPath: {}))\n", bin_note(*n), swift_str(BIN_FILE))),
        B::Form(pairs) => out.push_str(&format!("request.httpBody = Data({}.utf8)\n", swift_str(&Model::form_encoded(pairs)))),
        B::Multipart(parts) => {
            out.push_str("\nlet boundary = \"Boundary-\\(UUID().uuidString)\"\nrequest.setValue(\"multipart/form-data; boundary=\\(boundary)\", forHTTPHeaderField: \"Content-Type\")\nvar body = Data()\n");
            for p in parts {
                let head = multipart_head(&p.name, p.file.as_ref().map(|_| p.file_name.as_str()), p.content_type.as_deref());
                out.push_str(&format!("body.append(Data(\"--\\(boundary)\\r\\n\".utf8))\nbody.append(Data({}.utf8))\n", swift_str(&head)));
                match &p.file {
                    Some(f) => out.push_str(&format!("body.append(try Data(contentsOf: URL(fileURLWithPath: {})))\n", swift_str(f))),
                    None => out.push_str(&format!("body.append(Data({}.utf8))\n", swift_str(&p.value))),
                }
                out.push_str("body.append(Data(\"\\r\\n\".utf8))\n");
            }
            out.push_str("body.append(Data(\"--\\(boundary)--\\r\\n\".utf8))\nrequest.httpBody = body\n");
        }
    }
    out.push_str("\nlet (data, response) = try await URLSession.shared.data(for: request)\nif let http = response as? HTTPURLResponse {\n    print(http.statusCode)\n}\nprint(String(decoding: data, as: UTF8.self))\n");
    out
}

// ---------- PowerShell ----------

fn ps_body(s: &str) -> String {
    if s.contains('\n') && !s.lines().any(|l| l.starts_with("'@")) {
        format!("@'\n{s}\n'@")
    } else {
        ps_str(s)
    }
}

pub(crate) fn powershell(m: &Model) -> String {
    let mut out = String::new();
    let mut params: Vec<(String, String)> = vec![("Uri".into(), ps_str(&m.url))];
    match m.method.as_str() {
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE" => {
            let mut cs = m.method.chars();
            let first = cs.next().unwrap_or('G');
            params.push(("Method".into(), ps_str(&format!("{first}{}", cs.as_str().to_lowercase()))));
        }
        other => params.push(("CustomMethod".into(), ps_str(other))),
    }
    let headers: Vec<(String, String)> = m
        .folded_headers()
        .into_iter()
        .filter(|(k, v)| {
            if k.eq_ignore_ascii_case("content-type") && !matches!(m.body, B::None) {
                params.push(("ContentType".into(), ps_str(v)));
                false
            } else if k.eq_ignore_ascii_case("user-agent") {
                params.push(("UserAgent".into(), ps_str(v)));
                false
            } else {
                true
            }
        })
        .collect();
    if !headers.is_empty() {
        let rows: Vec<String> = headers.iter().map(|(k, v)| format!("    {} = {}", ps_str(k), ps_str(v))).collect();
        out.push_str(&format!("$headers = @{{\n{}\n}}\n", rows.join("\n")));
        params.push(("Headers".into(), "$headers".into()));
    }
    match &m.body {
        B::None => {}
        B::Text { text, .. } => {
            out.push_str(&format!("$body = {}\n", ps_body(text)));
            params.push(("Body".into(), "$body".into()));
        }
        B::Binary(n) => {
            out.push_str(&format!("# {}\n", bin_note(*n)));
            params.push(("InFile".into(), ps_str(BIN_FILE)));
        }
        B::Form(pairs) => params.push(("Body".into(), ps_str(&Model::form_encoded(pairs)))),
        B::Multipart(parts) => {
            let rows: Vec<String> = parts
                .iter()
                .map(|p| match &p.file {
                    Some(f) => format!("    {} = Get-Item -Path {}", ps_str(&p.name), ps_str(f)),
                    None => format!("    {} = {}", ps_str(&p.name), ps_str(&p.value)),
                })
                .collect();
            out.push_str(&format!("# -Form needs PowerShell 7+\n$form = @{{\n{}\n}}\n", rows.join("\n")));
            params.push(("Form".into(), "$form".into()));
        }
    }
    if m.insecure {
        params.push(("SkipCertificateCheck".into(), "$true".into()));
    }
    if !m.follow {
        params.push(("MaximumRedirection".into(), "0".into()));
    }
    if let Some(t) = m.timeout_ms {
        params.push(("TimeoutSec".into(), t.div_ceil(1000).max(1).to_string()));
    }
    if let Some(p) = &m.proxy {
        params.push(("Proxy".into(), ps_str(p)));
    }
    if let Some((u, p)) = &m.digest {
        out.push_str(&format!("$credential = New-Object System.Management.Automation.PSCredential({}, (ConvertTo-SecureString {} -AsPlainText -Force))\n", ps_str(u), ps_str(p)));
        params.push(("Credential".into(), "$credential".into()));
    }
    let width = params.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let rows: Vec<String> = params.iter().map(|(k, v)| format!("    {k:width$} = {v}")).collect();
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!("$params = @{{\n{}\n}}\n\n$response = Invoke-RestMethod @params\n$response\n", rows.join("\n")));
    out
}

// ---------- HTTPie ----------

fn httpie_escape_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, ':' | '=' | '@' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub(crate) fn httpie(m: &Model) -> String {
    let mut flags: Vec<String> = Vec::new();
    if m.insecure {
        flags.push("--verify=no".into());
    }
    if m.follow {
        flags.push("--follow".into());
    }
    if let Some(t) = m.timeout_ms {
        flags.push(format!("--timeout={}", seconds(t)));
    }
    if let Some(p) = &m.proxy {
        let scheme = if m.url.starts_with("https") { "https" } else { "http" };
        flags.push(sh_word(&format!("--proxy={scheme}:{p}")));
    }
    if let Some((u, p)) = &m.digest {
        flags.push("--auth-type=digest".into());
        flags.push(format!("--auth={}", sh_word(&format!("{u}:{p}"))));
    }
    let mut items: Vec<String> = Vec::new();
    for (k, v) in &m.headers {
        if matches!(m.body, B::Form(_)) && k.eq_ignore_ascii_case("content-type") {
            continue; // --form sets it
        }
        items.push(sh_word(&if v.is_empty() { format!("{k};") } else { format!("{k}:{v}") }));
    }
    let mut tail = String::new();
    match &m.body {
        B::None => {}
        B::Text { text, .. } => flags.push(format!("--raw {}", sh_word(text))),
        B::Binary(n) => tail = format!(" < {BIN_FILE}  # {}", bin_note(*n)),
        B::Form(pairs) => {
            flags.push("--form".into());
            items.extend(pairs.iter().map(|(k, v)| sh_word(&format!("{}={v}", httpie_escape_name(k)))));
        }
        B::Multipart(parts) => {
            flags.push("--multipart".into());
            for p in parts {
                items.push(sh_word(&match &p.file {
                    Some(f) => {
                        let ct = p.content_type.as_ref().map(|c| format!(";type={c}")).unwrap_or_default();
                        format!("{}@{f}{ct}", httpie_escape_name(&p.name))
                    }
                    None => format!("{}={}", httpie_escape_name(&p.name), p.value),
                }));
            }
        }
    }
    let mut first = String::from("http");
    for f in &flags {
        first.push(' ');
        first.push_str(f);
    }
    first.push_str(&format!(" {} {}", m.method, sh_word(&m.url)));
    let mut parts: Vec<String> = vec![first];
    parts.extend(items);
    let mut out = parts.join(" \\\n  ");
    out.push_str(&tail);
    out.push('\n');
    out
}

// ---------- Hurl ----------

fn hurl_kv(s: &str) -> String {
    s.replace('\\', "\\\\").replace('#', "\\#")
}

/// A Hurl entry (<https://hurl.dev>): request line, headers, `[Form]` / `[Multipart]`,
/// `[Options]` and the body. Run with `hurl --test file.hurl`.
pub(crate) fn hurl(m: &Model) -> String {
    let mut out = format!("{} {}\n", m.method, m.url);
    for (k, v) in &m.headers {
        out.push_str(&format!("{}: {}\n", hurl_kv(k), hurl_kv(v)));
    }
    match &m.body {
        B::Form(pairs) => {
            out.push_str("[Form]\n");
            for (k, v) in pairs {
                out.push_str(&format!("{}: {}\n", hurl_kv(k), hurl_kv(v)));
            }
        }
        B::Multipart(parts) => {
            out.push_str("[Multipart]\n");
            for p in parts {
                match &p.file {
                    Some(f) => {
                        let ct = p.content_type.as_deref().map(|c| format!(" {c}")).unwrap_or_default();
                        out.push_str(&format!("{}: file,{f};{ct}\n", hurl_kv(&p.name)));
                    }
                    None => out.push_str(&format!("{}: {}\n", hurl_kv(&p.name), hurl_kv(&p.value))),
                }
            }
        }
        _ => {}
    }
    let mut options: Vec<String> = Vec::new();
    if m.follow {
        options.push("location: true".into());
    }
    if m.insecure {
        options.push("insecure: true".into());
    }
    if let Some(p) = &m.proxy {
        options.push(format!("proxy: {p}"));
    }
    if let Some((u, p)) = &m.digest {
        options.push("digest: true".into());
        options.push(format!("user: {}:{}", hurl_kv(u), hurl_kv(p)));
    }
    if let Some((u, p)) = &m.ntlm {
        options.push("ntlm: true".into());
        options.push(format!("user: {}:{}", hurl_kv(u), hurl_kv(p)));
    }
    if let Some(t) = m.timeout_ms {
        options.push(format!("max-time: {t}ms"));
    }
    if !options.is_empty() {
        out.push_str("[Options]\n");
        for o in options {
            out.push_str(&o);
            out.push('\n');
        }
    }
    match &m.body {
        B::Text { text, json } => {
            let lang = if json.is_some() { "json" } else { "" };
            if text.contains("```") {
                out.push_str(&format!("{}\n", serde_json::to_string(text).unwrap_or_default()));
            } else {
                out.push_str(&format!("```{lang}\n{}\n```\n", text.trim_end_matches(['\r', '\n'])));
            }
        }
        B::Binary(n) => out.push_str(&format!("file,{BIN_FILE}; # {}\n", bin_note(*n))),
        _ => {}
    }
    out.push_str("HTTP *\n");
    out
}
