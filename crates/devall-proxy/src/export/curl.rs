//! "Copy as cURL" for a captured request, rendered by the HTTP client's exporter so quoting and
//! layout match the rest of the app. Compressed request bodies are decoded first.

use devall_http::{PreparedBody, PreparedRequest, to_curl as render};

use super::{REDACTED, decoded_body, is_sensitive_header};
use crate::model::Exchange;
use crate::util::is_hop_by_hop;

/// A bash-compatible cURL command. Sensitive header values are masked unless
/// `include_sensitive`.
pub fn to_curl(ex: &Exchange, include_sensitive: bool) -> String {
    let headers = ex
        .request
        .headers
        .iter()
        .filter(|(k, _)| !is_hop_by_hop(k) && !k.eq_ignore_ascii_case("host") && !k.eq_ignore_ascii_case("content-length") && !k.starts_with(':'))
        .filter(|(k, _)| !(k.eq_ignore_ascii_case("content-encoding") && ex.request.body.size > 0))
        .map(|(k, v)| (k.clone(), if !include_sensitive && is_sensitive_header(k) { REDACTED.to_string() } else { v.clone() }))
        .collect();
    let body = if ex.request.body.size == 0 {
        PreparedBody::None
    } else {
        PreparedBody::Bytes(decoded_body(&ex.request.body, &ex.request.headers).unwrap_or_default())
    };
    let req = PreparedRequest {
        method: ex.request.method.clone(),
        url: ex.request.url.clone(),
        headers,
        body,
        timeout_ms: 30_000,
        follow_redirects: false,
        verify_tls: true,
        proxy: None,
        client_cert: None,
        digest: None,
        ntlm: None,
    };
    render(&req)
}
