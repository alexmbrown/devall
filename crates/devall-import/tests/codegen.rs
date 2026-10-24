//! Snapshot tests: every language × a set of representative requests. Snapshots live in
//! `tests/snapshots/codegen/<language>.txt`; regenerate with `UPDATE_SNAPSHOTS=1 cargo test`.

use std::collections::BTreeSet;
use std::path::Path;

use devall_core::vars::NoSecrets;
use devall_core::{Auth, Body, HttpRequestFile, KeyValue, MultipartPart, Resolver};
use devall_http::{PreparedRequest, prepare};
use devall_import::codegen::{self, ALL, Language};

fn prepared_in(req: HttpRequestFile, root: &Path) -> PreparedRequest {
    let mut r = Resolver::new(&NoSecrets);
    prepare(&req, &mut r, root).unwrap()
}

fn prepared(req: HttpRequestFile) -> PreparedRequest {
    prepared_in(req, Path::new("/project"))
}

fn cases(binary_root: &Path) -> Vec<(&'static str, PreparedRequest)> {
    let mut get = HttpRequestFile::new("get");
    get.url = "https://api.example.com/v1/users?page=2".into();
    get.params.push(KeyValue::new("q", "ada lovelace"));
    get.headers.push(KeyValue::new("Accept", "application/json"));
    get.headers.push(KeyValue::new("Authorization", "Bearer ••••••"));
    get.headers.push(KeyValue::new("X-Trace", "a\"b'c$d"));

    let mut json = HttpRequestFile::new("json");
    json.method = "POST".into();
    json.url = "https://api.example.com/v1/users".into();
    json.body = Body::Json { content: "{\n  \"name\": \"Zoë \\\"Z\\\" O'Neil\",\n  \"path\": \"C:\\\\tmp\",\n  \"note\": \"line1\\nline2 ${HOME} #{x} $x\",\n  \"tags\": [\"a\", \"b\"],\n  \"active\": true,\n  \"score\": 1.5,\n  \"parent\": null\n}".into() };

    let mut form = HttpRequestFile::new("form");
    form.method = "PUT".into();
    form.url = "https://api.example.com/v1/settings".into();
    form.body = Body::Form { fields: vec![KeyValue::new("tag", "a&b"), KeyValue::new("tag", "c d"), KeyValue::new("mode", "dark")] };

    let mut multipart = HttpRequestFile::new("multipart");
    multipart.method = "POST".into();
    multipart.url = "https://api.example.com/v1/upload".into();
    multipart.headers.push(KeyValue::new("X-Upload-Id", "42"));
    multipart.body = Body::Multipart {
        parts: vec![
            MultipartPart { name: "note".into(), value: "hello 'world'".into(), file: None, content_type: None, enabled: true },
            MultipartPart { name: "file".into(), value: String::new(), file: Some("files/a b.png".into()), content_type: Some("image/png".into()), enabled: true },
        ],
    };

    let mut options = HttpRequestFile::new("options");
    options.method = "DELETE".into();
    options.url = "https://internal.example.com/v1/cache".into();
    options.auth = Auth::Digest { username: "admin".into(), password: "••••••".into() };
    options.settings.verify_tls = false;
    options.settings.follow_redirects = false;
    options.settings.timeout_ms = 2500;
    options.settings.proxy = Some("http://proxy.local:3128".into());

    let mut text = HttpRequestFile::new("text");
    text.method = "PURGE".into();
    text.url = "https://cdn.example.com/assets/app.js".into();
    text.body = Body::Text { content: "line 1 with 'quotes' and \"double\"\n'@ here-string end\n\"\"\" triple\n\\ backslash `tick` ${x}".into(), content_type: None };

    std::fs::write(binary_root.join("payload.bin"), [0xffu8, 0x00, 0x10, 0x80]).unwrap();
    let mut binary = HttpRequestFile::new("binary");
    binary.method = "POST".into();
    binary.url = "https://api.example.com/v1/blobs".into();
    binary.body = Body::Binary { path: "payload.bin".into(), content_type: Some("application/octet-stream".into()) };

    vec![
        ("get-query", prepared(get)),
        ("post-json", prepared(json)),
        ("form", prepared(form)),
        ("multipart", prepared(multipart)),
        ("options", prepared(options)),
        ("text-custom-method", prepared(text)),
        ("binary", prepared_in(binary, binary_root)),
    ]
}

fn id(lang: Language) -> String {
    serde_json::to_value(lang).unwrap().as_str().unwrap().to_string()
}

#[test]
fn snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let cases = cases(tmp.path());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots").join("codegen");
    let update = std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    let mut failures = Vec::new();
    for &lang in ALL {
        let mut text = String::new();
        for (name, req) in &cases {
            text.push_str(&format!("===== {name} =====\n"));
            text.push_str(&codegen::generate(req, lang));
        }
        let file = dir.join(format!("{}.txt", id(lang)));
        if update || !file.exists() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&file, &text).unwrap();
            if !update {
                failures.push(format!("{}: new snapshot written; review it and run again", file.display()));
            }
            continue;
        }
        let expected = std::fs::read_to_string(&file).unwrap().replace("\r\n", "\n");
        if expected != text {
            failures.push(format!("{} differs (UPDATE_SNAPSHOTS=1 to accept):\n{text}", file.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_language_covers_the_essentials() {
    let tmp = tempfile::tempdir().unwrap();
    let cases = cases(tmp.path());
    let infos = codegen::languages();
    assert_eq!(infos.len(), 16);
    for &lang in ALL {
        let get = codegen::generate(&cases[0].1, lang);
        assert!(get.contains("api.example.com/v1/users"), "{lang:?}:\n{get}");
        assert!(get.contains("Bearer ••••••"), "{lang:?}:\n{get}");
        assert!(!get.contains("devall/"), "default User-Agent is left out ({lang:?})");
        let multipart = codegen::generate(&cases[3].1, lang);
        assert!(multipart.contains("/project/files/a b.png"), "{lang:?} file path:\n{multipart}");
        let json = codegen::generate(&cases[1].1, lang);
        assert!(json.contains("Zo") && json.contains("Neil"), "{lang:?}:\n{json}");
        let options = codegen::generate(&cases[4].1, lang);
        assert!(options.contains("internal.example.com"), "{lang:?}");
    }
}

#[test]
fn json_snippets_parse_back_in_python_and_javascript_form() {
    let tmp = tempfile::tempdir().unwrap();
    let cases = cases(tmp.path());
    let js = codegen::generate(&cases[1].1, Language::JavascriptFetch);
    let start = js.find("JSON.stringify(").unwrap() + "JSON.stringify(".len();
    let end = js.rfind("),").unwrap();
    let embedded: serde_json::Value = serde_json::from_str(&js[start..end]).unwrap();
    let original: serde_json::Value = match &cases[1].1.body {
        devall_http::PreparedBody::Bytes(b) => serde_json::from_slice(b).unwrap(),
        _ => unreachable!(),
    };
    assert_eq!(embedded, original);
    let py = codegen::generate(&cases[1].1, Language::PythonRequests);
    assert!(py.contains("\"active\": True") && py.contains("\"parent\": None") && py.contains("json=payload"), "{py}");
}

#[test]
fn redaction_masks_every_encoding_of_a_secret() {
    let mut req = HttpRequestFile::new("s");
    req.url = "https://x.io/a".into();
    req.params.push(KeyValue::new("key", "p@ss w0rd/&"));
    req.headers.push(KeyValue::new("X-Key", "p@ss w0rd/&"));
    req.auth = Auth::Basic { username: "bob".into(), password: "p@ss w0rd/&".into() };
    req.body = Body::Json { content: "{\"k\": \"p@ss w0rd/&\"}".into() };
    let p = prepared(req);
    let secrets: BTreeSet<String> = ["p@ss w0rd/&".to_string()].into_iter().collect();
    let masked = codegen::redact(&p, &secrets);
    for &lang in ALL {
        let code = codegen::generate(&masked, lang);
        assert!(!code.contains("p@ss") && !code.contains("p%40ss") && !code.contains("cEBzcyB3MHJkLyY"), "{lang:?} leaks:\n{code}");
        let b64 = base64_of("bob:p@ss w0rd/&");
        assert!(!code.contains(&b64), "{lang:?} leaks basic auth");
    }
}

fn base64_of(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s)
}

#[test]
fn ntlm_is_native_in_curl_and_noted_elsewhere() {
    let mut req = HttpRequestFile::new("ntlm");
    req.url = "https://intranet.example.com/api".into();
    req.auth = Auth::Ntlm { username: "ada".into(), password: "••••••".into(), domain: Some("CORP".into()), workstation: None };
    let p = prepared(req);
    for &lang in ALL {
        let code = codegen::generate(&p, lang);
        if lang == Language::Curl {
            assert!(code.contains("--ntlm --user") && code.contains("ada:••••••") && code.matches("--ntlm").count() == 1, "{code}");
        } else if lang == Language::Hurl {
            // Hurl escapes the backslash in `DOMAIN\user`.
            assert!(code.contains(r"ntlm: true
user: CORP\\ada:••••••"), "{code}");
        } else {
            assert!(code.lines().next().unwrap().contains("NTLM auth (user CORP"), "{}: {code}", id(lang));
        }
    }
}
