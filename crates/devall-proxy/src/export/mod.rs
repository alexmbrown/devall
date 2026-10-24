//! Turning captured traffic into project artifacts: HAR, WireMock stubs, saved requests, an
//! OpenAPI draft, a load-test scenario and cURL commands.

pub mod curl;
pub mod har;
pub mod load;
pub mod openapi;
pub mod requests;
pub mod wiremock;

use crate::body::decode_body;
use crate::model::{CapturedBody, HeaderList};
use crate::util::header_value;

/// Header values masked in exports unless the user opts in.
pub const SENSITIVE_HEADERS: &[&str] = &["authorization", "proxy-authorization", "cookie", "set-cookie", "x-api-key", "x-auth-token", "x-csrf-token"];

pub const REDACTED: &str = "REDACTED";

pub fn is_sensitive_header(name: &str) -> bool {
    SENSITIVE_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// The body with `Content-Encoding` undone. `None` when the body was not captured (evicted) or
/// cannot be decoded.
pub(crate) fn decoded_body(body: &CapturedBody, headers: &HeaderList) -> Option<Vec<u8>> {
    if body.raw.is_empty() {
        return (body.size == 0 && !body.evicted).then(Vec::new);
    }
    match header_value(headers, "content-encoding").map(str::trim).filter(|e| !e.is_empty() && !e.eq_ignore_ascii_case("identity")) {
        Some(enc) => decode_body(&body.raw, enc).ok(),
        None => Some(body.raw.to_vec()),
    }
}

/// `content-type` → `Content-Type`.
pub(crate) fn canonical_header(name: &str) -> String {
    name.split('-')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Decoded `(name, value)` query pairs of a URL.
pub(crate) fn query_pairs(url: &str) -> Vec<(String, String)> {
    url::Url::parse(url).map(|u| u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()).unwrap_or_default()
}
