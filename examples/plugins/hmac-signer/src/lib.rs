//! Example /dev/all plugin.
//!
//! - Auth provider `hmac`: signs each request with HMAC-SHA256 over
//!   `METHOD \n PATH?QUERY \n TIMESTAMP \n NONCE \n SHA256(body)` and adds
//!   `Authorization: HMAC-SHA256 keyId="…", ts="…", nonce="…", signature="…"` plus
//!   `X-Content-SHA256`. The key comes from the auth config (`{{secret.X}}` works) or, when that is
//!   empty, from the plugin secret `HMAC_SECRET` (granted on the Plugins page).
//! - Utility `sign`: HMAC-SHA256 of the input text (hex or Base64).
//!
//! Build: `cargo build --target wasm32-unknown-unknown --release`, then copy
//! `target/wasm32-unknown-unknown/release/hmac_signer.wasm` to `plugin.wasm` next to `plugin.yaml`.

use base64::Engine;
use devall_plugin::*;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

pub struct Signer;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// `/path?query` of an absolute URL (no fragment).
pub fn path_and_query(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split('#').next().unwrap_or_default();
    match rest.find(['/', '?']) {
        Some(i) if rest[i..].starts_with('?') => format!("/{}", &rest[i..]),
        Some(i) => rest[i..].to_string(),
        None => "/".into(),
    }
}

/// The string to sign and its signature (Base64) — pure, so it is unit-tested on the host.
pub fn sign(method: &str, url: &str, body: &[u8], ts: i64, nonce: &str, key: &[u8]) -> (String, String, String) {
    let body_hash = hex::encode(Sha256::digest(body));
    let to_sign = format!("{}\n{}\n{ts}\n{nonce}\n{body_hash}", method.to_ascii_uppercase(), path_and_query(url));
    let signature = base64::engine::general_purpose::STANDARD.encode(hmac(key, to_sign.as_bytes()));
    (to_sign, signature, body_hash)
}

fn key_from(config_value: Option<&String>) -> Result<Vec<u8>, String> {
    match config_value.filter(|v| !v.is_empty()) {
        Some(v) => Ok(v.as_bytes().to_vec()),
        None => host::secret("HMAC_SECRET").map(String::into_bytes).map_err(|e| format!("no key in the auth config and HMAC_SECRET is unavailable ({e})")),
    }
}

impl Plugin for Signer {
    fn utility(&self, id: &str, input: UtilityInput) -> Result<UtilityOutput, String> {
        if id != "sign" {
            return Err(format!("unknown utility {id}"));
        }
        let key = key_from(input.args.get("key"))?;
        let mac = hmac(&key, input.input.as_bytes());
        Ok(UtilityOutput::text(match input.args.get("encoding").map(String::as_str) {
            Some("base64") => base64::engine::general_purpose::STANDARD.encode(mac),
            _ => hex::encode(mac),
        }))
    }

    fn auth(&self, id: &str, input: AuthInput) -> Result<AuthOutput, String> {
        if id != "hmac" {
            return Err(format!("unknown auth provider {id}"));
        }
        let cfg = &input.config;
        let key_id = cfg.get("keyId").filter(|k| !k.is_empty()).ok_or("keyId is required")?;
        let key = key_from(cfg.get("secret"))?;
        let header = cfg.get("header").filter(|h| !h.is_empty()).map(String::as_str).unwrap_or("Authorization");
        let body = match (&input.request.body, input.request.body_base64) {
            (Some(b), true) => base64::engine::general_purpose::STANDARD.decode(b).map_err(|e| e.to_string())?,
            (Some(b), false) => b.clone().into_bytes(),
            (None, _) => Vec::new(),
        };
        let (now_ms, _) = host::now()?;
        let nonce = host::random_hex(8)?;
        let ts = now_ms / 1000;
        let (_, signature, body_hash) = sign(&input.request.method, &input.request.url, &body, ts, &nonce, &key);
        host::log(host::Level::Debug, &format!("signed {} {} at {ts}", input.request.method, path_and_query(&input.request.url)));
        Ok(AuthOutput {
            headers: vec![
                Header::new(header, format!("HMAC-SHA256 keyId=\"{key_id}\", ts=\"{ts}\", nonce=\"{nonce}\", signature=\"{signature}\"")),
                Header::new("X-Content-SHA256", body_hash),
            ],
            query: Vec::new(),
        })
    }
}

devall_plugin::export!(Signer);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(path_and_query("https://api.example.com/v1/items?a=1#frag"), "/v1/items?a=1");
        assert_eq!(path_and_query("https://api.example.com"), "/");
        assert_eq!(path_and_query("https://api.example.com?x=1"), "/?x=1");
    }

    #[test]
    fn signature_is_stable() {
        let (to_sign, sig, hash) = sign("post", "https://api.example.com/v1/items?a=1", b"{}", 1_700_000_000, "abcd", b"secret");
        assert_eq!(to_sign, format!("POST\n/v1/items?a=1\n1700000000\nabcd\n{hash}"));
        assert_eq!(hash, "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a");
        assert_eq!(sig.len(), 44);
        assert_eq!(sig, sign("POST", "https://api.example.com/v1/items?a=1", b"{}", 1_700_000_000, "abcd", b"secret").1);
    }

    #[test]
    fn utility_through_dispatch() {
        let env = r#"{"apiVersion":1,"kind":"utility","id":"sign","input":{"input":"hello","args":{"key":"k","encoding":"hex"}}}"#;
        let out = dispatch(&Signer, env.as_bytes());
        assert_eq!(out["ok"]["output"].as_str().unwrap().len(), 64);
        // Outside /dev/all the auth provider can't get a timestamp from the host.
        let env = r#"{"apiVersion":1,"kind":"auth","id":"hmac","input":{"request":{"method":"GET","url":"https://x/"},"config":{"keyId":"k1","secret":"s"}}}"#;
        assert!(dispatch(&Signer, env.as_bytes())["error"].as_str().unwrap().contains("only available inside /dev/all"));
    }
}
