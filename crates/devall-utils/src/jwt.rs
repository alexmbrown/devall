//! JWT decode / verify / sign. Decoding is done by hand so malformed or unsigned tokens can still
//! be inspected; verification and signing use `jsonwebtoken`.

use std::collections::HashSet;

use chrono::{TimeZone, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{Map, Value, json};

use crate::{Result, decode_base64, invalid};

fn part(token: &str, idx: usize) -> Result<Value> {
    let seg = token.split('.').nth(idx).ok_or_else(|| invalid("a JWT has three dot-separated parts"))?;
    let bytes = decode_base64(seg)?;
    serde_json::from_slice(&bytes).map_err(|e| invalid(format!("part {} is not JSON: {e}", idx + 1)))
}

fn strip_bearer(t: &str) -> &str {
    t.strip_prefix("Bearer ").or_else(|| t.strip_prefix("bearer ")).unwrap_or(t).trim()
}

pub fn decode(token: &str) -> Result<Value> {
    decode_at(token, Utc::now().timestamp())
}

pub(crate) fn decode_at(token: &str, now: i64) -> Result<Value> {
    let token = strip_bearer(token);
    if token.split('.').count() != 3 {
        return Err(invalid("a JWT has three dot-separated parts (header.payload.signature)"));
    }
    let header = part(token, 0)?;
    let payload = part(token, 1)?;
    let mut times = Map::new();
    let mut warnings: Vec<String> = Vec::new();
    for claim in ["iat", "nbf", "exp", "auth_time"] {
        if let Some(ts) = payload.get(claim).and_then(Value::as_i64) {
            let iso = Utc.timestamp_opt(ts, 0).single().map(|d| d.to_rfc3339());
            times.insert(claim.into(), json!({ "unix": ts, "iso": iso, "relative": relative(ts - now) }));
        }
    }
    if let Some(exp) = payload.get("exp").and_then(Value::as_i64) {
        if exp <= now {
            warnings.push(format!("Expired {}", relative(exp - now)));
        } else if exp - now < 300 {
            warnings.push(format!("Expires soon ({})", relative(exp - now)));
        }
    } else {
        warnings.push("No exp claim: this token never expires".into());
    }
    if let Some(nbf) = payload.get("nbf").and_then(Value::as_i64)
        && nbf > now
    {
        warnings.push(format!("Not valid yet (nbf {})", relative(nbf - now)));
    }
    if header.get("alg").and_then(Value::as_str).is_some_and(|a| a.eq_ignore_ascii_case("none")) {
        warnings.push("alg is \"none\": the token is unsigned".into());
    }
    Ok(json!({
        "header": header,
        "payload": payload,
        "signature": token.rsplit('.').next().unwrap_or(""),
        "times": times,
        "warnings": warnings,
    }))
}

fn relative(delta: i64) -> String {
    let abs = delta.unsigned_abs();
    let (n, unit) = if abs < 60 {
        (abs, "s")
    } else if abs < 3600 {
        (abs / 60, "m")
    } else if abs < 86_400 {
        (abs / 3600, "h")
    } else {
        (abs / 86_400, "d")
    };
    if delta >= 0 { format!("in {n}{unit}") } else { format!("{n}{unit} ago") }
}

fn parse_alg(s: &str) -> Result<Algorithm> {
    s.trim().parse::<Algorithm>().map_err(|_| invalid(format!("unsupported algorithm {s}")))
}

fn is_hmac(a: Algorithm) -> bool {
    matches!(a, Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512)
}

fn decoding_key(alg: Algorithm, key: &str) -> Result<DecodingKey> {
    let bytes = key.as_bytes();
    let k = match alg {
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => return Ok(DecodingKey::from_secret(bytes)),
        Algorithm::ES256 | Algorithm::ES384 => DecodingKey::from_ec_pem(bytes),
        Algorithm::EdDSA => DecodingKey::from_ed_pem(bytes),
        _ => DecodingKey::from_rsa_pem(bytes),
    };
    k.map_err(|e| invalid(format!("cannot read PEM public key: {e}")))
}

fn encoding_key(alg: Algorithm, key: &str) -> Result<EncodingKey> {
    let bytes = key.as_bytes();
    let k = match alg {
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => return Ok(EncodingKey::from_secret(bytes)),
        Algorithm::ES256 | Algorithm::ES384 => EncodingKey::from_ec_pem(bytes),
        Algorithm::EdDSA => EncodingKey::from_ed_pem(bytes),
        _ => EncodingKey::from_rsa_pem(bytes),
    };
    k.map_err(|e| invalid(format!("cannot read PEM private key: {e}")))
}

pub fn verify(token: &str, key: &str, alg: &str) -> Result<Value> {
    let token = strip_bearer(token);
    let header = part(token, 0)?;
    let header_alg = header.get("alg").and_then(Value::as_str).unwrap_or("");
    let alg = parse_alg(if alg.trim().is_empty() { header_alg } else { alg })?;
    if key.is_empty() {
        return Err(invalid(if is_hmac(alg) { "enter the shared secret" } else { "paste the PEM public key" }));
    }
    let mut v = Validation::new(alg);
    v.required_spec_claims = HashSet::new();
    v.validate_aud = false;
    v.leeway = 0;
    match jsonwebtoken::decode::<Value>(token, &decoding_key(alg, key)?, &v) {
        Ok(_) => Ok(json!({ "valid": true, "algorithm": format!("{alg:?}") })),
        Err(e) => Ok(json!({ "valid": false, "algorithm": format!("{alg:?}"), "error": e.to_string() })),
    }
}

pub fn sign(payload: &str, key: &str, alg: &str) -> Result<String> {
    let alg = parse_alg(if alg.trim().is_empty() { "HS256" } else { alg })?;
    let claims: Value = serde_json::from_str(payload).map_err(|e| invalid(format!("payload must be a JSON object: {e}")))?;
    if !claims.is_object() {
        return Err(invalid("payload must be a JSON object"));
    }
    if key.is_empty() {
        return Err(invalid(if is_hmac(alg) { "enter a secret" } else { "paste a PEM private key" }));
    }
    jsonwebtoken::encode(&Header::new(alg), &claims, &encoding_key(alg, key)?).map_err(|e| invalid(e.to_string()))
}
