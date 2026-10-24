//! HTTP Digest authentication (RFC 7616, with RFC 2617 compatibility): parse the server's
//! `WWW-Authenticate: Digest …` challenge and compute the `Authorization` header.

use md5::Md5;
use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub algorithm: String,
    pub qop: Vec<String>,
}

/// Parses the first Digest challenge in a `WWW-Authenticate` value.
pub fn parse_challenge(header: &str) -> Option<Challenge> {
    let rest = header.trim().strip_prefix("Digest").or_else(|| header.trim().strip_prefix("digest"))?;
    let mut c = Challenge { algorithm: "MD5".into(), ..Default::default() };
    for (k, v) in parse_params(rest) {
        match k.to_ascii_lowercase().as_str() {
            "realm" => c.realm = v,
            "nonce" => c.nonce = v,
            "opaque" => c.opaque = Some(v),
            "algorithm" => c.algorithm = v,
            "qop" => c.qop = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            _ => {}
        }
    }
    (!c.nonce.is_empty()).then_some(c)
}

fn parse_params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i] == ',' || chars[i].is_whitespace()) {
            i += 1;
        }
        let start = i;
        while i < chars.len() && chars[i] != '=' {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect::<String>().trim().to_string();
        i += 1;
        let mut val = String::new();
        if i < chars.len() && chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                val.push(chars[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < chars.len() && chars[i] != ',' {
                val.push(chars[i]);
                i += 1;
            }
        }
        if !key.is_empty() {
            out.push((key, val.trim().to_string()));
        }
    }
    out
}

fn h(algorithm: &str, data: &str) -> String {
    if algorithm.to_ascii_uppercase().starts_with("SHA-256") {
        hex::encode(Sha256::digest(data.as_bytes()))
    } else {
        hex::encode(Md5::digest(data.as_bytes()))
    }
}

/// Builds the `Authorization` header value. `uri` is the request target (path + query).
pub fn authorization(c: &Challenge, method: &str, uri: &str, user: &str, pass: &str, cnonce: &str, nc: u32) -> String {
    let alg = c.algorithm.to_ascii_uppercase();
    let sess = alg.ends_with("-SESS");
    let mut ha1 = h(&alg, &format!("{user}:{}:{pass}", c.realm));
    if sess {
        ha1 = h(&alg, &format!("{ha1}:{}:{cnonce}", c.nonce));
    }
    let ha2 = h(&alg, &format!("{method}:{uri}"));
    let nc_hex = format!("{nc:08x}");
    let use_qop = c.qop.iter().any(|q| q == "auth");
    let response = if use_qop {
        h(&alg, &format!("{ha1}:{}:{nc_hex}:{cnonce}:auth:{ha2}", c.nonce))
    } else {
        h(&alg, &format!("{ha1}:{}:{ha2}", c.nonce))
    };
    let mut out = format!(
        r#"Digest username="{user}", realm="{}", nonce="{}", uri="{uri}", algorithm={}, response="{response}""#,
        c.realm, c.nonce, c.algorithm
    );
    if use_qop {
        out.push_str(&format!(r#", qop=auth, nc={nc_hex}, cnonce="{cnonce}""#));
    }
    if let Some(o) = &c.opaque {
        out.push_str(&format!(r#", opaque="{o}""#));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 2617 §3.5 example ("Mufasa").
    #[test]
    fn rfc2617_example() {
        let c = parse_challenge(r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#).unwrap();
        assert_eq!(c.realm, "testrealm@host.com");
        assert_eq!(c.qop, vec!["auth", "auth-int"]);
        let a = authorization(&c, "GET", "/dir/index.html", "Mufasa", "Circle Of Life", "0a4f113b", 1);
        assert!(a.contains(r#"response="6629fae49393a05397450978507c4ef1""#), "{a}");
        assert!(a.contains("nc=00000001"));
        assert!(a.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#));
    }

    #[test]
    fn sha256_and_no_qop() {
        let c = parse_challenge(r#"Digest realm="r", nonce="n", algorithm=SHA-256"#).unwrap();
        let a = authorization(&c, "POST", "/x", "u", "p", "c", 1);
        assert!(a.contains("algorithm=SHA-256"));
        assert!(!a.contains("qop="));
        assert!(parse_challenge("Basic realm=x").is_none());
    }
}
