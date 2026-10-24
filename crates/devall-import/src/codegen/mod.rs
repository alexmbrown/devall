//! Code generation from a resolved [`PreparedRequest`] (templates already applied; mask secrets
//! first with [`redact`] unless the user asked to include them).

use std::collections::BTreeSet;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use devall_http::{PreparedBody, PreparedRequest, USER_AGENT};

mod escape;
mod langs;

pub use devall_http::to_curl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "curl")]
    Curl,
    #[serde(rename = "javascript-fetch")]
    JavascriptFetch,
    #[serde(rename = "node-axios")]
    NodeAxios,
    #[serde(rename = "python-requests")]
    PythonRequests,
    #[serde(rename = "python-httpx")]
    PythonHttpx,
    #[serde(rename = "go")]
    Go,
    #[serde(rename = "rust-reqwest")]
    RustReqwest,
    #[serde(rename = "java-httpclient")]
    JavaHttpClient,
    #[serde(rename = "csharp-httpclient")]
    CsharpHttpClient,
    #[serde(rename = "php-curl")]
    PhpCurl,
    #[serde(rename = "ruby-net-http")]
    RubyNetHttp,
    #[serde(rename = "swift-urlsession")]
    SwiftUrlSession,
    #[serde(rename = "kotlin-okhttp")]
    KotlinOkHttp,
    #[serde(rename = "powershell")]
    PowerShell,
    #[serde(rename = "httpie")]
    Httpie,
    #[serde(rename = "hurl")]
    Hurl,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    pub id: Language,
    pub label: &'static str,
    /// Grouping for the picker (`Shell`, `JavaScript`, …).
    pub group: &'static str,
    /// Monaco language id for highlighting.
    pub editor: &'static str,
    pub extension: &'static str,
}

pub const ALL: &[Language] = &[
    Language::Curl,
    Language::Httpie,
    Language::PowerShell,
    Language::JavascriptFetch,
    Language::NodeAxios,
    Language::PythonRequests,
    Language::PythonHttpx,
    Language::Go,
    Language::RustReqwest,
    Language::JavaHttpClient,
    Language::KotlinOkHttp,
    Language::CsharpHttpClient,
    Language::PhpCurl,
    Language::RubyNetHttp,
    Language::SwiftUrlSession,
    Language::Hurl,
];

impl Language {
    pub fn info(self) -> LanguageInfo {
        let (label, group, editor, extension) = match self {
            Language::Curl => ("cURL", "Shell", "shell", "sh"),
            Language::Httpie => ("HTTPie", "Shell", "shell", "sh"),
            Language::PowerShell => ("PowerShell (Invoke-RestMethod)", "Shell", "powershell", "ps1"),
            Language::JavascriptFetch => ("JavaScript (fetch)", "JavaScript", "javascript", "js"),
            Language::NodeAxios => ("Node.js (axios)", "JavaScript", "javascript", "mjs"),
            Language::PythonRequests => ("Python (requests)", "Python", "python", "py"),
            Language::PythonHttpx => ("Python (httpx)", "Python", "python", "py"),
            Language::Go => ("Go (net/http)", "Go", "go", "go"),
            Language::RustReqwest => ("Rust (reqwest)", "Rust", "rust", "rs"),
            Language::JavaHttpClient => ("Java (HttpClient)", "JVM", "java", "java"),
            Language::KotlinOkHttp => ("Kotlin (OkHttp)", "JVM", "kotlin", "kt"),
            Language::CsharpHttpClient => ("C# (HttpClient)", ".NET", "csharp", "cs"),
            Language::PhpCurl => ("PHP (cURL)", "PHP", "php", "php"),
            Language::RubyNetHttp => ("Ruby (Net::HTTP)", "Ruby", "ruby", "rb"),
            Language::SwiftUrlSession => ("Swift (URLSession)", "Swift", "swift", "swift"),
            Language::Hurl => ("Hurl", "Test files", "plaintext", "hurl"),
        };
        LanguageInfo { id: self, label, group, editor, extension }
    }
}

/// Every supported language, in picker order.
pub fn languages() -> Vec<LanguageInfo> {
    ALL.iter().map(|l| l.info()).collect()
}

/// Renders `req` as a snippet in `lang`.
pub fn generate(req: &PreparedRequest, lang: Language) -> String {
    let m = Model::new(req);
    let mut out = match lang {
        Language::Curl => curl(req, &m),
        Language::Httpie => langs::httpie(&m),
        Language::PowerShell => langs::powershell(&m),
        Language::JavascriptFetch => langs::fetch(&m),
        Language::NodeAxios => langs::axios(&m),
        Language::PythonRequests => langs::python(&m, false),
        Language::PythonHttpx => langs::python(&m, true),
        Language::Go => langs::go(&m),
        Language::RustReqwest => langs::rust(&m),
        Language::JavaHttpClient => langs::java(&m),
        Language::KotlinOkHttp => langs::kotlin(&m),
        Language::CsharpHttpClient => langs::csharp(&m),
        Language::PhpCurl => langs::php(&m),
        Language::RubyNetHttp => langs::ruby(&m),
        Language::SwiftUrlSession => langs::swift(&m),
        Language::Hurl => langs::hurl(&m),
    };
    if let Some((user, _)) = &m.ntlm
        && !matches!(lang, Language::Curl | Language::Hurl)
    {
        out.insert_str(0, &ntlm_note(lang, user));
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// None of the generated clients speak NTLM out of the box: say so at the top of the snippet.
fn ntlm_note(lang: Language, user: &str) -> String {
    let hint = match lang {
        Language::PythonRequests => "use requests_ntlm.HttpNtlmAuth",
        Language::PythonHttpx => "use httpx-ntlm's HttpNtlmAuth",
        Language::PowerShell => "pass -Credential (Get-Credential) or -UseDefaultCredentials",
        Language::CsharpHttpClient => "use new HttpClientHandler { Credentials = new NetworkCredential(user, password, domain) }",
        Language::Httpie => "install httpie-ntlm and pass --auth-type=ntlm",
        Language::JavaHttpClient | Language::KotlinOkHttp => "use an NTLM-capable client (e.g. Apache HttpClient's NTCredentials)",
        Language::PhpCurl => "set CURLOPT_HTTPAUTH => CURLAUTH_NTLM with CURLOPT_USERPWD",
        _ => "answer the NTLM challenge with an NTLM library",
    };
    let comment = if matches!(lang, Language::Httpie | Language::PowerShell | Language::PythonRequests | Language::PythonHttpx | Language::RubyNetHttp) { "#" } else { "//" };
    format!("{comment} NTLM auth (user {user}) is not built into this client: {hint}.\n")
}

fn curl(req: &PreparedRequest, m: &Model) -> String {
    // Forward slashes work on every platform and keep snippets identical across them.
    let mut req = req.clone();
    if let PreparedBody::Multipart(parts) = &mut req.body {
        for p in parts {
            if let Some(f) = &mut p.file {
                *f = std::path::PathBuf::from(f.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    // Auth handshakes are appended below in this snippet's own style.
    req.digest = None;
    req.ntlm = None;
    let mut cmd = to_curl(&req);
    if let Some((u, p)) = &m.digest {
        cmd.push_str(&format!(" \\\n  --digest --user {}", escape::sh_word(&format!("{u}:{p}"))));
    }
    if let Some((u, p)) = &m.ntlm {
        cmd.push_str(&format!(" \\\n  --ntlm --user {}", escape::sh_word(&format!("{u}:{p}"))));
    }
    if let Some(t) = m.timeout_ms {
        cmd.push_str(&format!(" \\\n  --max-time {}", seconds(t)));
    }
    if let Some(p) = &m.proxy {
        cmd.push_str(&format!(" \\\n  --proxy {}", escape::sh_word(p)));
    }
    cmd
}

/// `2500` → `2.5`, `30000` → `30`.
pub(crate) fn seconds(ms: u64) -> String {
    if ms.is_multiple_of(1000) { (ms / 1000).to_string() } else { format!("{}", ms as f64 / 1000.0) }
}

/// Replaces secret values (and their URL-/JSON-encoded and Basic-auth forms) with `••••••`.
pub fn redact(req: &PreparedRequest, secrets: &BTreeSet<String>) -> PreparedRequest {
    const MASK: &str = "••••••";
    let mut variants: Vec<(String, &str)> = Vec::new();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        variants.push((s.clone(), MASK));
        let enc: String = url::form_urlencoded::byte_serialize(s.as_bytes()).collect();
        if enc != *s {
            variants.push((enc, MASK));
        }
        let json = serde_json::to_string(s).unwrap_or_default();
        let json = json.trim_matches('"').to_string();
        if json != *s {
            variants.push((json, MASK));
        }
    }
    variants.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
    let mask = |text: &str| {
        let mut t = text.to_string();
        for (v, m) in &variants {
            if t.contains(v.as_str()) {
                t = t.replace(v.as_str(), m);
            }
        }
        t
    };
    let mut out = req.clone();
    out.url = mask(&out.url);
    for (k, v) in &mut out.headers {
        // `Authorization: Basic base64(user:pass)` hides the secret from plain matching.
        if let Some(b64) = v.strip_prefix("Basic ")
            && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim())
            && let Ok(text) = String::from_utf8(decoded)
            && secrets.iter().any(|s| s.len() >= 4 && text.contains(s.as_str()))
        {
            *v = format!("Basic {MASK}");
            continue;
        }
        *k = mask(k);
        *v = mask(v);
    }
    out.body = match &req.body {
        PreparedBody::Bytes(b) => match std::str::from_utf8(b) {
            Ok(t) => PreparedBody::Bytes(mask(t).into_bytes()),
            Err(_) => PreparedBody::Bytes(b.clone()),
        },
        PreparedBody::Form(pairs) => PreparedBody::Form(pairs.iter().map(|(k, v)| (k.clone(), mask(v))).collect()),
        PreparedBody::Multipart(parts) => PreparedBody::Multipart(
            parts
                .iter()
                .map(|p| {
                    let mut p = p.clone();
                    p.value = mask(&p.value);
                    p
                })
                .collect(),
        ),
        other => other.clone(),
    };
    if let Some((_, pass)) = &mut out.digest
        && secrets.contains(pass.as_str())
    {
        *pass = MASK.to_string();
    }
    if let Some(c) = &mut out.ntlm
        && secrets.contains(c.password.as_str())
    {
        c.password = MASK.to_string();
    }
    out
}

// ---------- the language-neutral model every generator renders ----------

pub(crate) struct Part {
    pub name: String,
    pub value: String,
    /// Forward-slash path (valid on Windows too).
    pub file: Option<String>,
    pub file_name: String,
    pub content_type: Option<String>,
}

pub(crate) enum B {
    None,
    /// UTF-8 body; `json` is set when it is a JSON object/array sent as JSON.
    Text {
        text: String,
        json: Option<Value>,
    },
    /// Non-UTF-8 bytes (length): snippets read it from a file.
    Binary(usize),
    Form(Vec<(String, String)>),
    Multipart(Vec<Part>),
}

pub(crate) struct Model {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: B,
    pub insecure: bool,
    pub follow: bool,
    pub timeout_ms: Option<u64>,
    pub digest: Option<(String, String)>,
    /// `DOMAIN\user` and password.
    pub ntlm: Option<(String, String)>,
    pub proxy: Option<String>,
}

impl Model {
    pub(crate) fn new(req: &PreparedRequest) -> Self {
        let mut headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter(|(k, v)| {
                let l = k.to_ascii_lowercase();
                !((l == "user-agent" && v == USER_AGENT) || (l == "accept" && v == "*/*") || l == "host" || l == "content-length")
            })
            .cloned()
            .collect();
        let ct = req.header("content-type").map(str::to_ascii_lowercase);
        let body = match &req.body {
            PreparedBody::None => B::None,
            PreparedBody::Bytes(b) => match std::str::from_utf8(b) {
                Ok(t) => {
                    let json = ct
                        .as_deref()
                        .filter(|c| c.contains("json"))
                        .and_then(|_| serde_json::from_str::<Value>(t).ok())
                        .filter(|v| v.is_object() || v.is_array());
                    B::Text { text: t.to_string(), json }
                }
                Err(_) => B::Binary(b.len()),
            },
            PreparedBody::Form(pairs) => {
                if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                    headers.push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
                }
                B::Form(pairs.clone())
            }
            PreparedBody::Multipart(parts) => {
                // Every client writes its own boundary.
                headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
                B::Multipart(
                    parts
                        .iter()
                        .map(|p| {
                            let file = p.file.as_ref().map(|f| f.to_string_lossy().replace('\\', "/"));
                            let file_name = file.as_deref().map(|f| crate::util::file_name_of(f).to_string()).unwrap_or_default();
                            Part { name: p.name.clone(), value: p.value.clone(), file, file_name, content_type: p.content_type.clone() }
                        })
                        .collect(),
                )
            }
        };
        Model {
            method: req.method.to_uppercase(),
            url: req.url.clone(),
            headers,
            body,
            insecure: !req.verify_tls,
            follow: req.follow_redirects,
            timeout_ms: (req.timeout_ms > 0 && req.timeout_ms != 30_000).then_some(req.timeout_ms),
            digest: req.digest.clone(),
            ntlm: req.ntlm.as_ref().map(|c| (c.qualified_user(), c.password.clone())),
            proxy: req.proxy.clone().filter(|p| p != "none"),
        }
    }

    pub(crate) fn content_type(&self) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.as_str())
    }

    /// Headers with duplicates folded (`a, b`) for map-based APIs.
    pub(crate) fn folded_headers(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for (k, v) in &self.headers {
            match out.iter_mut().find(|(x, _)| x.eq_ignore_ascii_case(k)) {
                Some((_, existing)) => {
                    existing.push_str(", ");
                    existing.push_str(v);
                }
                None => out.push((k.clone(), v.clone())),
            }
        }
        out
    }

    pub(crate) fn has_duplicate_headers(&self) -> bool {
        self.folded_headers().len() != self.headers.len()
    }

    /// The form body encoded as `a=1&b=2`.
    pub(crate) fn form_encoded(pairs: &[(String, String)]) -> String {
        url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish()
    }

    pub(crate) fn has_files(&self) -> bool {
        matches!(&self.body, B::Multipart(parts) if parts.iter().any(|p| p.file.is_some())) || matches!(self.body, B::Binary(_))
    }
}
