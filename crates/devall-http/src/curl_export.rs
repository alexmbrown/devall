use crate::{PreparedBody, PreparedRequest, USER_AGENT};

/// Renders a prepared request as a bash-compatible cURL command. Headers we add by default
/// (our User-Agent, `Accept: */*`) are omitted to keep the command clean.
pub fn to_curl(req: &PreparedRequest) -> String {
    let mut parts: Vec<String> = vec!["curl".into()];
    let implicit_get = req.method == "GET" && matches!(req.body, PreparedBody::None);
    if !implicit_get {
        parts.push(format!("-X {}", req.method));
    }
    parts.push(quote(&req.url));
    for (k, v) in &req.headers {
        let default = (k.eq_ignore_ascii_case("user-agent") && v == USER_AGENT) || (k.eq_ignore_ascii_case("accept") && v == "*/*");
        if !default {
            parts.push(format!("-H {}", quote(&format!("{k}: {v}"))));
        }
    }
    match &req.body {
        PreparedBody::None => {}
        PreparedBody::Bytes(b) => match std::str::from_utf8(b) {
            Ok(t) => parts.push(format!("--data-raw {}", quote(t))),
            Err(_) => parts.push("--data-binary @body.bin".into()),
        },
        PreparedBody::Form(pairs) => {
            for (k, v) in pairs {
                parts.push(format!("--data-urlencode {}", quote(&format!("{k}={v}"))));
            }
        }
        PreparedBody::Multipart(ps) => {
            for p in ps {
                let mut v = match &p.file {
                    Some(f) => format!("{}=@{}", p.name, f.display()),
                    None => format!("{}={}", p.name, p.value),
                };
                if let Some(ct) = &p.content_type {
                    v.push_str(&format!(";type={ct}"));
                }
                parts.push(format!("-F {}", quote(&v)));
            }
        }
    }
    // Challenge-response schemes: curl runs the handshake itself.
    if let Some((user, pass)) = &req.digest {
        parts.push(format!("--digest -u {}", quote(&format!("{user}:{pass}"))));
    }
    if let Some(c) = &req.ntlm {
        parts.push(format!("--ntlm -u {}", quote(&format!("{}:{}", c.qualified_user(), c.password))));
    }
    if !req.verify_tls {
        parts.push("--insecure".into());
    }
    if req.follow_redirects {
        parts.push("--location".into());
    }
    parts.join(" \\\n  ")
}

fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}
