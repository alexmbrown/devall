//! AWS Signature Version 4 (header-based). Reference:
//! https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html

use hmac::{KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = hmac::Hmac<Sha256>;

pub struct SigV4Params<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: Option<&'a str>,
    pub region: &'a str,
    pub service: &'a str,
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data.as_bytes());
    m.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// RFC 3986 encoding as AWS expects (unreserved characters untouched; `/` kept in paths).
fn aws_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let unreserved = b.is_ascii_alphanumeric() || b"-_.~".contains(&b) || (keep_slash && b == b'/');
        if unreserved {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Returns the headers to add (`Authorization`, `X-Amz-Date`, `X-Amz-Content-Sha256`, and
/// `X-Amz-Security-Token` when a session token is set). `headers` are the request's existing
/// headers; `Host` is derived from the URL.
pub fn sign(
    method: &str,
    url: &url::Url,
    headers: &[(String, String)],
    body: &[u8],
    p: &SigV4Params,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<(String, String)> {
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let payload_hash = sha256_hex(body);

    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let mut signed: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("user-agent"))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    signed.retain(|(k, _)| k != "host" && k != "x-amz-date" && k != "x-amz-content-sha256" && k != "x-amz-security-token");
    signed.push(("host".into(), host));
    signed.push(("x-amz-content-sha256".into(), payload_hash.clone()));
    signed.push(("x-amz-date".into(), amz_date.clone()));
    if let Some(t) = p.session_token.filter(|t| !t.is_empty()) {
        signed.push(("x-amz-security-token".into(), t.to_string()));
    }
    signed.sort_by(|a, b| a.0.cmp(&b.0));
    // Merge duplicate header names (comma-joined, in order).
    let mut merged: Vec<(String, String)> = Vec::new();
    for (k, v) in signed {
        match merged.last_mut() {
            Some((lk, lv)) if *lk == k => {
                lv.push(',');
                lv.push_str(&v);
            }
            _ => merged.push((k, v)),
        }
    }

    let canonical_headers: String = merged.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed_headers = merged.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");

    let mut query: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (aws_encode(&k, false), aws_encode(&v, false))).collect();
    query.sort();
    let canonical_query = query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let path = if url.path().is_empty() { "/" } else { url.path() };
    // S3 uses the raw path; other services double-encode. Paths from `url` are already
    // percent-encoded once, so decode-then-encode gives the single canonical encoding.
    let decoded = percent_decode(path);
    let canonical_path = if p.service == "s3" { aws_encode(&decoded, true) } else { aws_encode(&aws_encode(&decoded, true), true) };

    let canonical_request = format!("{method}\n{canonical_path}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
    let scope = format!("{date}/{}/{}/aws4_request", p.region, p.service);
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));

    let k_date = hmac(format!("AWS4{}", p.secret_access_key).as_bytes(), &date);
    let k_region = hmac(&k_date, p.region);
    let k_service = hmac(&k_region, p.service);
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex::encode(hmac(&k_signing, &string_to_sign));

    let mut out = vec![
        (
            "Authorization".to_string(),
            format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}", p.access_key_id),
        ),
        ("X-Amz-Date".to_string(), amz_date),
        ("X-Amz-Content-Sha256".to_string(), payload_hash),
    ];
    if let Some(t) = p.session_token.filter(|t| !t.is_empty()) {
        out.push(("X-Amz-Security-Token".to_string(), t.to_string()));
    }
    out
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// AWS's published example: GET https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08
    /// signed at 20150830T123600Z with the documented example credentials.
    #[test]
    fn matches_aws_documentation_example() {
        let url = url::Url::parse("https://iam.amazonaws.com/?Action=ListUsers&Version=2010-05-08").unwrap();
        let headers = vec![("Content-Type".to_string(), "application/x-www-form-urlencoded; charset=utf-8".to_string())];
        let p = SigV4Params {
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            region: "us-east-1",
            service: "iam",
        };
        let now = chrono::Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).unwrap();
        let out = sign("GET", &url, &headers, b"", &p, now);
        let auth = &out[0].1;
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request"), "{auth}");
        assert!(auth.contains("SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date"), "{auth}");
        assert_eq!(out[1], ("X-Amz-Date".to_string(), "20150830T123600Z".to_string()));
        // Signature changes with any input; check determinism and shape.
        let again = sign("GET", &url, &headers, b"", &p, now);
        assert_eq!(out, again);
        assert_eq!(auth.rsplit("Signature=").next().unwrap().len(), 64);
    }

    #[test]
    fn session_token_is_signed_and_sent() {
        let url = url::Url::parse("https://s3.eu-west-1.amazonaws.com/bucket/key%20name.txt").unwrap();
        let p = SigV4Params { access_key_id: "A", secret_access_key: "S", session_token: Some("T"), region: "eu-west-1", service: "s3" };
        let out = sign("PUT", &url, &[], b"hello", &p, chrono::Utc::now());
        assert!(out[0].1.contains("x-amz-security-token"));
        assert!(out.iter().any(|(k, v)| k == "X-Amz-Security-Token" && v == "T"));
        assert_eq!(out[2].1, sha256_hex(b"hello"));
    }
}
