//! Rust functions exposed to the prelude as `__devall_native.*` (removed from the global scope
//! before user code runs). Binary data crosses the boundary as lowercase hex strings.

use std::cell::RefCell;
use std::rc::Rc;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_PAD_INDIFFERENT, URL_SAFE_NO_PAD, URL_SAFE_PAD_INDIFFERENT};
use hmac::{KeyInit, Mac};
use rquickjs::{Ctx, Exception, Function, Object};
use sha2::Digest;

use crate::{LogLevel, LogLine, ScriptPhase};

/// Upper bound for `crypto.getRandomValues` / `WordArray.random` requests.
const MAX_RANDOM_BYTES: usize = 1024 * 1024;

pub(crate) fn digest(alg: &str, data: &[u8]) -> Option<Vec<u8>> {
    Some(match alg {
        "md5" => md5::Md5::digest(data).to_vec(),
        "sha1" => sha1::Sha1::digest(data).to_vec(),
        "sha224" => sha2::Sha224::digest(data).to_vec(),
        "sha256" => sha2::Sha256::digest(data).to_vec(),
        "sha384" => sha2::Sha384::digest(data).to_vec(),
        "sha512" => sha2::Sha512::digest(data).to_vec(),
        _ => return None,
    })
}

fn mac<M: Mac + KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = <M as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

pub(crate) fn hmac(alg: &str, key: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    use hmac::Hmac;
    Some(match alg {
        "md5" => mac::<Hmac<md5::Md5>>(key, data),
        "sha1" => mac::<Hmac<sha1::Sha1>>(key, data),
        "sha224" => mac::<Hmac<sha2::Sha224>>(key, data),
        "sha256" => mac::<Hmac<sha2::Sha256>>(key, data),
        "sha384" => mac::<Hmac<sha2::Sha384>>(key, data),
        "sha512" => mac::<Hmac<sha2::Sha512>>(key, data),
        _ => return None,
    })
}

/// Lenient base64: standard or URL-safe alphabet, padding optional, ASCII whitespace ignored.
pub(crate) fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let clean: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if clean.contains(['-', '_']) { URL_SAFE_PAD_INDIFFERENT.decode(&clean).ok() } else { STANDARD_PAD_INDIFFERENT.decode(&clean).ok() }
}

/// Console output collected while a script runs (survives timeouts and crashes).
pub(crate) struct LogSink {
    pub lines: Vec<LogLine>,
    phase: ScriptPhase,
    source: String,
    max_lines: usize,
    max_len: usize,
    dropped: usize,
}

impl LogSink {
    pub fn new(phase: ScriptPhase, source: &str, max_lines: usize, max_len: usize) -> Self {
        Self { lines: Vec::new(), phase, source: source.to_string(), max_lines, max_len, dropped: 0 }
    }

    pub fn push(&mut self, level: LogLevel, mut message: String) {
        if self.lines.len() >= self.max_lines {
            self.dropped += 1;
            return;
        }
        if message.len() > self.max_len {
            let cut = message.floor_char_boundary(self.max_len);
            let total = message.len();
            message.truncate(cut);
            message.push_str(&format!("… ({total} bytes)"));
        }
        self.lines.push(LogLine { phase: self.phase, source: self.source.clone(), level, message });
    }

    pub fn finish(mut self) -> Vec<LogLine> {
        if self.dropped > 0 {
            let n = self.dropped;
            self.max_lines += 1;
            self.push(LogLevel::Warn, format!("… {n} more console line(s) dropped (limit {})", self.max_lines - 1));
        }
        self.lines
    }
}

fn type_error(ctx: &Ctx<'_>, msg: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, msg)
}

fn decode_hex(ctx: &Ctx<'_>, h: &str) -> rquickjs::Result<Vec<u8>> {
    hex::decode(h).map_err(|_| type_error(ctx, "invalid hex string"))
}

/// Builds the `__devall_native` object.
pub(crate) fn install<'js>(ctx: &Ctx<'js>, sink: Rc<RefCell<LogSink>>) -> rquickjs::Result<Object<'js>> {
    let n = Object::new(ctx.clone())?;

    n.set(
        "log",
        Function::new(ctx.clone(), move |level: String, text: String| {
            sink.borrow_mut().push(LogLevel::parse(&level), text);
        })?,
    )?;
    n.set(
        "hash",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, alg: String, data: String| -> rquickjs::Result<String> {
            let data = decode_hex(&ctx, &data)?;
            digest(&alg, &data).map(hex::encode).ok_or_else(|| type_error(&ctx, &format!("unsupported hash algorithm {alg}")))
        })?,
    )?;
    n.set(
        "hmac",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, alg: String, key: String, data: String| -> rquickjs::Result<String> {
            let key = decode_hex(&ctx, &key)?;
            let data = decode_hex(&ctx, &data)?;
            hmac(&alg, &key, &data).map(hex::encode).ok_or_else(|| type_error(&ctx, &format!("unsupported HMAC algorithm {alg}")))
        })?,
    )?;
    n.set("utf8ToHex", Function::new(ctx.clone(), |s: String| hex::encode(s.as_bytes()))?)?;
    n.set(
        "hexToUtf8",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, h: String| -> rquickjs::Result<String> { Ok(String::from_utf8_lossy(&decode_hex(&ctx, &h)?).into_owned()) })?,
    )?;
    n.set(
        "hexToB64",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, h: String, url_safe: bool| -> rquickjs::Result<String> {
            let bytes = decode_hex(&ctx, &h)?;
            Ok(if url_safe { URL_SAFE_NO_PAD.encode(bytes) } else { STANDARD.encode(bytes) })
        })?,
    )?;
    n.set(
        "b64ToHex",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: String| -> rquickjs::Result<String> {
            b64_decode(&s).map(hex::encode).ok_or_else(|| type_error(&ctx, "invalid base64 string"))
        })?,
    )?;
    n.set(
        "btoa",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: String| -> rquickjs::Result<String> {
            let mut bytes = Vec::with_capacity(s.len());
            for c in s.chars() {
                let code = c as u32;
                if code > 0xFF {
                    return Err(Exception::throw_dom(&ctx, "InvalidCharacterError", "btoa: the string contains characters outside of the Latin1 range"));
                }
                bytes.push(code as u8);
            }
            Ok(STANDARD.encode(bytes))
        })?,
    )?;
    n.set(
        "atob",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, s: String| -> rquickjs::Result<String> {
            let bytes = b64_decode(&s).ok_or_else(|| Exception::throw_dom(&ctx, "InvalidCharacterError", "atob: the string is not correctly base64-encoded"))?;
            Ok(bytes.into_iter().map(char::from).collect())
        })?,
    )?;
    n.set("uuid", Function::new(ctx.clone(), || uuid::Uuid::new_v4().to_string())?)?;
    n.set(
        "randomHex",
        Function::new(ctx.clone(), |ctx: Ctx<'js>, len: f64| -> rquickjs::Result<String> {
            if !(0.0..=MAX_RANDOM_BYTES as f64).contains(&len) {
                return Err(Exception::throw_range(&ctx, &format!("random byte count must be between 0 and {MAX_RANDOM_BYTES}")));
            }
            let mut buf = vec![0u8; len as usize];
            getrandom::fill(&mut buf).map_err(|e| Exception::throw_internal(&ctx, &format!("random source failed: {e}")))?;
            Ok(hex::encode(buf))
        })?,
    )?;
    n.set("dynamic", Function::new(ctx.clone(), |name: String| devall_core::vars::dynamic(&name))?)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // RFC 4231 test case 2 and well-known digests.
        let h = hmac("sha256", b"Jefe", b"what do ya want for nothing?").unwrap();
        assert_eq!(hex::encode(h), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        assert_eq!(hex::encode(digest("md5", b"").unwrap()), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex::encode(digest("sha1", b"abc").unwrap()), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert!(digest("sha3", b"").is_none());
        assert_eq!(b64_decode("aGk").unwrap(), b"hi");
        assert_eq!(b64_decode("-_8=").unwrap(), vec![0xfb, 0xff]);
    }
}
