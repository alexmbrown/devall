//! Small self-contained utilities: gzip, string escaping, number bases, extra hashes, bcrypt,
//! passwords, lorem ipsum, QR codes, TOTP, slugs, HTTP status codes, user agents and chmod.

use std::io::{Read, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{KeyInit, Mac};
use serde_json::{Value, json};
use sha3::Digest;

use crate::{Result, arg_bool, arg_str, arg_usize, decode_base64, invalid};

// ── Gzip ─────────────────────────────────────────────────────────────────────────────────────────

/// Decompressed output cap (zip-bomb guard).
const MAX_INFLATE: u64 = 64 << 20;

pub fn gzip_compress(input: &str, level: u32) -> Result<String> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    enc.write_all(input.as_bytes()).map_err(|e| invalid(e.to_string()))?;
    Ok(STANDARD.encode(enc.finish().map_err(|e| invalid(e.to_string()))?))
}

pub(crate) fn inflate_any(bytes: &[u8]) -> Result<Vec<u8>> {
    fn read_all(r: impl Read) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        r.take(MAX_INFLATE).read_to_end(&mut out)?;
        Ok(out)
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return read_all(flate2::read::MultiGzDecoder::new(bytes)).map_err(|e| invalid(format!("not valid gzip: {e}")));
    }
    if bytes.len() > 2
        && bytes[0] & 0x0f == 8
        && (u16::from(bytes[0]) << 8 | u16::from(bytes[1])).is_multiple_of(31)
        && let Ok(v) = read_all(flate2::read::ZlibDecoder::new(bytes))
    {
        return Ok(v);
    }
    read_all(flate2::read::DeflateDecoder::new(bytes)).map_err(|_| invalid("not gzip, zlib or raw deflate data"))
}

pub fn gzip_decompress(input: &str) -> Result<String> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return Err(invalid("paste Base64 (or hex) gzip data"));
    }
    let bytes = if compact.len().is_multiple_of(2) && compact.chars().all(|c| c.is_ascii_hexdigit()) && compact.to_ascii_lowercase().starts_with("1f8b") {
        hex::decode(&compact).map_err(|e| invalid(e.to_string()))?
    } else {
        decode_base64(&compact)?
    };
    Ok(crate::bytes_to_text(inflate_any(&bytes)?))
}

// ── String escaping ──────────────────────────────────────────────────────────────────────────────

fn backslash_escape(input: &str, quote: char, unicode_style: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\0' if unicode_style == "c" => out.push_str("\\0"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => match unicode_style {
                "c" => out.push_str(&format!("\\x{:02x}", c as u32)),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            },
            // Java/JS string literals can't hold U+2028/2029 safely in older engines.
            '\u{2028}' | '\u{2029}' if unicode_style != "c" => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn backslash_unescape(input: &str) -> Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut it = input.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = it.next() else {
            out.push('\\');
            break;
        };
        match e {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'v' => out.push('\u{b}'),
            'a' => out.push('\u{7}'),
            '0' if !it.peek().is_some_and(|c| c.is_digit(8)) => out.push('\0'),
            'x' => {
                let h: String = it.by_ref().take(2).collect();
                let v = u32::from_str_radix(&h, 16).map_err(|_| invalid(format!("bad \\x escape: \\x{h}")))?;
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            'u' => {
                let code = if it.peek() == Some(&'{') {
                    it.next();
                    let h: String = it.by_ref().take_while(|c| *c != '}').collect();
                    u32::from_str_radix(&h, 16).map_err(|_| invalid(format!("bad \\u{{{h}}} escape")))?
                } else {
                    let h: String = it.by_ref().take(4).collect();
                    let hi = u32::from_str_radix(&h, 16).map_err(|_| invalid(format!("bad \\u escape: \\u{h}")))?;
                    // Surrogate pair 😀.
                    if (0xD800..0xDC00).contains(&hi) {
                        let mut look = it.clone();
                        if look.next() == Some('\\') && look.next() == Some('u') {
                            let l: String = look.by_ref().take(4).collect();
                            if let Ok(lo) = u32::from_str_radix(&l, 16)
                                && (0xDC00..0xE000).contains(&lo)
                            {
                                it = look;
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            }
                        } else {
                            hi
                        }
                    } else {
                        hi
                    }
                };
                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
            }
            d if d.is_digit(8) => {
                let mut s = d.to_string();
                while s.len() < 3 && it.peek().is_some_and(|c| c.is_digit(8)) {
                    s.push(it.next().unwrap_or('0'));
                }
                out.push(char::from_u32(u32::from_str_radix(&s, 8).unwrap_or(0xfffd)).unwrap_or('\u{fffd}'));
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

pub fn string_escape(input: &str, mode: &str, unescape: bool) -> Result<String> {
    Ok(match (mode, unescape) {
        ("json", false) => {
            let s = serde_json::to_string(input).unwrap_or_default();
            s[1..s.len() - 1].to_string()
        }
        ("json", true) => {
            let t = input.trim();
            let quoted = if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 { t.to_string() } else { format!("\"{t}\"") };
            serde_json::from_str::<String>(&quoted).map_err(|e| invalid(format!("not a valid JSON string: {e}")))?
        }
        ("js", false) => backslash_escape(input, '\'', "js").replace('"', "\\\""),
        ("java", false) => backslash_escape(input, '"', "java"),
        ("c", false) => backslash_escape(input, '"', "c"),
        ("js" | "java" | "c", true) => backslash_unescape(input)?,
        ("sql", false) => input.replace('\'', "''"),
        ("sql", true) => input.replace("''", "'"),
        ("regex", false) => regex::escape(input),
        ("regex", true) => {
            let mut out = String::new();
            let mut it = input.chars();
            while let Some(c) = it.next() {
                if c == '\\' {
                    if let Some(n) = it.next() {
                        out.push(n);
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        // POSIX shell: wrap in single quotes; a literal ' becomes '\''.
        ("shell", false) => format!("'{}'", input.replace('\'', "'\\''")),
        ("shell", true) => {
            let t = input.trim();
            if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
                t[1..t.len() - 1].replace("'\\''", "'")
            } else if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
                backslash_unescape(&t[1..t.len() - 1])?
            } else {
                backslash_unescape(t)?
            }
        }
        ("csv", false) => {
            if input.contains([',', '"', '\n', '\r']) || input.starts_with(' ') || input.ends_with(' ') {
                format!("\"{}\"", input.replace('"', "\"\""))
            } else {
                input.to_string()
            }
        }
        ("csv", true) => {
            let t = input.trim();
            if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 { t[1..t.len() - 1].replace("\"\"", "\"") } else { t.to_string() }
        }
        ("xml", false) => input.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;"),
        ("xml", true) => html_escape::decode_html_entities(input).into_owned(),
        (other, _) => return Err(invalid(format!("unknown mode {other}"))),
    })
}

// ── Number bases ─────────────────────────────────────────────────────────────────────────────────

const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

fn to_base(mut n: u128, base: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % base as u128) as usize]);
        n /= base as u128;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

fn group(s: &str, size: usize, sep: char) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(size) {
            out.push(sep);
        }
        out.push(*c);
    }
    out
}

pub fn number_base(input: &str, from: &str, to: u32) -> Result<Value> {
    let raw: String = input.trim().chars().filter(|c| !matches!(c, '_' | ' ' | ',' | '\'')).collect();
    if raw.is_empty() {
        return Err(invalid("enter a number, e.g. 255, 0xff, 0b1010 or 0o755"));
    }
    let (neg, body) = match raw.strip_prefix('-') {
        Some(b) => (true, b.to_string()),
        None => (false, raw.trim_start_matches('+').to_string()),
    };
    let lower = body.to_ascii_lowercase();
    let (base, digits) = match from.trim() {
        "" | "auto" => {
            if let Some(d) = lower.strip_prefix("0x") {
                (16, d.to_string())
            } else if let Some(d) = lower.strip_prefix("0b") {
                (2, d.to_string())
            } else if let Some(d) = lower.strip_prefix("0o") {
                (8, d.to_string())
            } else if lower.chars().all(|c| c.is_ascii_digit()) {
                (10, lower.clone())
            } else if lower.chars().all(|c| c.is_ascii_hexdigit()) {
                (16, lower.clone())
            } else {
                return Err(invalid("can't tell the base; set \"Input base\" (2–36)"));
            }
        }
        b => {
            let base: u32 = b.parse().map_err(|_| invalid(format!("bad base {b}")))?;
            if !(2..=36).contains(&base) {
                return Err(invalid("base must be between 2 and 36"));
            }
            let d = match base {
                16 => lower.strip_prefix("0x").unwrap_or(&lower),
                2 => lower.strip_prefix("0b").unwrap_or(&lower),
                8 => lower.strip_prefix("0o").unwrap_or(&lower),
                _ => &lower,
            };
            (base, d.to_string())
        }
    };
    let n = u128::from_str_radix(&digits, base).map_err(|e| invalid(format!("not a base-{base} number (max 128 bits): {e}")))?;
    let sign = if neg { "-" } else { "" };
    let mut out = json!({
        "inputBase": base,
        "decimal": format!("{sign}{n}"),
        "hex": format!("{sign}0x{}", to_base(n, 16)),
        "octal": format!("{sign}0o{}", to_base(n, 8)),
        "binary": format!("{sign}0b{}", to_base(n, 2)),
        "binaryGrouped": format!("{sign}{}", group(&to_base(n, 2), 4, ' ')),
        "decimalGrouped": format!("{sign}{}", group(&n.to_string(), 3, ',')),
        "bits": 128 - n.leading_zeros(),
    });
    if ![2, 8, 10, 16].contains(&to) {
        out[format!("base{to}")] = json!(format!("{sign}{}", to_base(n, to)));
    }
    if neg && n <= 1u128 << 63 {
        let v = (n as i128).wrapping_neg() as i64;
        out["twosComplement64"] = json!(format!("0x{:016x}", v as u64));
    }
    if !neg && n <= u32::MAX as u128 {
        out["asIpv4"] = json!(std::net::Ipv4Addr::from(n as u32).to_string());
    }
    Ok(out)
}

// ── Hashes / bcrypt ──────────────────────────────────────────────────────────────────────────────

pub fn sha3(input: &str, variant: &str) -> Result<String> {
    let b = input.as_bytes();
    Ok(match variant {
        "224" => hex::encode(sha3::Sha3_224::digest(b)),
        "384" => hex::encode(sha3::Sha3_384::digest(b)),
        "512" => hex::encode(sha3::Sha3_512::digest(b)),
        "keccak256" => hex::encode(sha3::Keccak256::digest(b)),
        "" | "256" => hex::encode(sha3::Sha3_256::digest(b)),
        other => return Err(invalid(format!("unknown SHA-3 variant {other}"))),
    })
}

pub fn blake3_hex(input: &str, len: usize) -> String {
    let mut out = vec![0u8; len];
    blake3::Hasher::new().update(input.as_bytes()).finalize_xof().fill(&mut out);
    hex::encode(out)
}

pub fn bcrypt_hash(password: &str, cost: u32) -> Result<String> {
    if !(4..=16).contains(&cost) {
        return Err(invalid("cost must be between 4 and 16 (each step doubles the time; 10–12 is typical)"));
    }
    if password.len() > 72 {
        return Err(invalid("bcrypt only uses the first 72 bytes of a password; this one is longer"));
    }
    bcrypt::hash(password, cost).map_err(|e| invalid(e.to_string()))
}

pub fn bcrypt_verify(password: &str, hash: &str) -> Result<Value> {
    let hash = hash.trim();
    if hash.is_empty() {
        return Err(invalid("paste the bcrypt hash ($2b$…) into the hash field"));
    }
    let valid = bcrypt::verify(password, hash).map_err(|e| invalid(format!("not a bcrypt hash: {e}")))?;
    let cost = hash.split('$').nth(2).and_then(|c| c.parse::<u32>().ok());
    Ok(json!({ "valid": valid, "cost": cost, "variant": hash.split('$').nth(1) }))
}

// ── Randomness ───────────────────────────────────────────────────────────────────────────────────

/// Uniform integer in 0..n from the OS CSPRNG (rejection sampling, no modulo bias).
fn rand_below(n: u32) -> Result<u32> {
    let zone = u32::MAX - (u32::MAX % n);
    loop {
        let v = getrandom::u32().map_err(|e| invalid(format!("no OS randomness: {e}")))?;
        if v < zone {
            return Ok(v % n);
        }
    }
}

const AMBIGUOUS: &str = "Il1O0o|`'\"";

pub fn password_generate(args: &Value) -> Result<Value> {
    let length = arg_usize(args, "length", 20, 4, 1024);
    let count = arg_usize(args, "count", 5, 1, 100);
    let no_ambiguous = arg_bool(args, "excludeAmbiguous");
    let on = |k: &str| arg_str(args, k) != "no";
    let mut classes: Vec<Vec<char>> = Vec::new();
    for (key, set) in [("lower", "abcdefghijklmnopqrstuvwxyz"), ("upper", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"), ("digits", "0123456789"), ("symbols", "!@#$%^&*()-_=+[]{};:,.<>/?~")] {
        if on(key) {
            classes.push(set.chars().filter(|c| !(no_ambiguous && AMBIGUOUS.contains(*c))).collect());
        }
    }
    if classes.is_empty() {
        return Err(invalid("enable at least one character class"));
    }
    if classes.len() > length {
        return Err(invalid("the length is shorter than the number of character classes"));
    }
    let alphabet: Vec<char> = classes.concat();
    let mut passwords = Vec::new();
    for _ in 0..count {
        // Resample until every enabled class is present (keeps the distribution uniform over valid passwords).
        loop {
            let pw: Vec<char> = (0..length).map(|_| rand_below(alphabet.len() as u32).map(|i| alphabet[i as usize])).collect::<Result<_>>()?;
            if classes.iter().all(|cls| pw.iter().any(|c| cls.contains(c))) {
                passwords.push(pw.into_iter().collect::<String>());
                break;
            }
        }
    }
    let entropy = length as f64 * (alphabet.len() as f64).log2();
    let strength = match entropy as u32 {
        0..=39 => "weak",
        40..=63 => "fair",
        64..=79 => "strong",
        _ => "very strong",
    };
    Ok(json!({
        "passwords": passwords,
        "length": length,
        "alphabetSize": alphabet.len(),
        "entropyBits": (entropy * 10.0).round() / 10.0,
        "strength": strength,
    }))
}

const LOREM: &[&str] = &[
    "lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do", "eiusmod", "tempor", "incididunt", "ut", "labore", "et", "dolore", "magna", "aliqua", "enim", "ad", "minim", "veniam",
    "quis", "nostrud", "exercitation", "ullamco", "laboris", "nisi", "aliquip", "ex", "ea", "commodo", "consequat", "duis", "aute", "irure", "in", "reprehenderit", "voluptate", "velit", "esse", "cillum", "fugiat",
    "nulla", "pariatur", "excepteur", "sint", "occaecat", "cupidatat", "non", "proident", "sunt", "culpa", "qui", "officia", "deserunt", "mollit", "anim", "id", "est", "laborum", "praesent", "vitae", "ligula",
    "vestibulum", "tortor", "mauris", "viverra", "nunc", "sapien", "pellentesque", "habitant", "morbi", "tristique", "senectus", "netus", "fames", "turpis", "egestas", "integer", "feugiat", "scelerisque", "varius",
    "facilisis", "gravida", "cursus", "metus", "aliquam", "eleifend", "porta", "blandit",
];

struct Lorem;

impl Lorem {
    fn word(&self) -> Result<&'static str> {
        Ok(LOREM[rand_below(LOREM.len() as u32)? as usize])
    }

    fn sentence(&self) -> Result<String> {
        let n = 6 + rand_below(10)? as usize;
        let mut words: Vec<String> = (0..n).map(|_| self.word().map(str::to_string)).collect::<Result<_>>()?;
        if n > 9 {
            let comma = 3 + rand_below((n - 6) as u32)? as usize;
            words[comma].push(',');
        }
        let mut s = words.join(" ");
        if let Some(f) = s.get(..1) {
            s = f.to_uppercase() + &s[1..];
        }
        Ok(s + ".")
    }

    fn paragraph(&self) -> Result<String> {
        let n = 4 + rand_below(4)? as usize;
        Ok((0..n).map(|_| self.sentence()).collect::<Result<Vec<_>>>()?.join(" "))
    }
}

pub fn lorem(unit: &str, count: usize) -> Result<String> {
    let l = Lorem;
    Ok(match unit {
        "words" => {
            let mut words = vec!["lorem", "ipsum"];
            while words.len() < count {
                words.push(l.word()?);
            }
            words.truncate(count);
            words.join(" ")
        }
        "sentences" => (0..count).map(|_| l.sentence()).collect::<Result<Vec<_>>>()?.join(" "),
        _ => {
            let mut paras: Vec<String> = (0..count).map(|_| l.paragraph()).collect::<Result<_>>()?;
            if let Some(first) = paras.first_mut() {
                *first = format!("Lorem ipsum dolor sit amet, consectetur adipiscing elit. {first}");
            }
            paras.join("\n\n")
        }
    })
}

// ── QR ───────────────────────────────────────────────────────────────────────────────────────────

pub fn qr_svg(input: &str, ec: &str, size: u32) -> Result<String> {
    if input.is_empty() {
        return Err(invalid("enter the text or URL to encode"));
    }
    let level = match ec {
        "L" => qrcode::EcLevel::L,
        "Q" => qrcode::EcLevel::Q,
        "H" => qrcode::EcLevel::H,
        _ => qrcode::EcLevel::M,
    };
    let code = qrcode::QrCode::with_error_correction_level(input.as_bytes(), level).map_err(|e| invalid(format!("cannot encode as QR: {e}")))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(size, size)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

// ── TOTP (RFC 6238) ──────────────────────────────────────────────────────────────────────────────

fn hotp(secret: &[u8], counter: u64, digits: u32, alg: &str) -> Result<String> {
    let msg = counter.to_be_bytes();
    let mac: Vec<u8> = match alg {
        "SHA256" => {
            let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret).map_err(|e| invalid(e.to_string()))?;
            m.update(&msg);
            m.finalize().into_bytes().to_vec()
        }
        "SHA512" => {
            let mut m = hmac::Hmac::<sha2::Sha512>::new_from_slice(secret).map_err(|e| invalid(e.to_string()))?;
            m.update(&msg);
            m.finalize().into_bytes().to_vec()
        }
        _ => {
            let mut m = hmac::Hmac::<sha1::Sha1>::new_from_slice(secret).map_err(|e| invalid(e.to_string()))?;
            m.update(&msg);
            m.finalize().into_bytes().to_vec()
        }
    };
    let off = (mac[mac.len() - 1] & 0x0f) as usize;
    let bin = u32::from_be_bytes([mac[off] & 0x7f, mac[off + 1], mac[off + 2], mac[off + 3]]);
    let code = bin % 10u32.pow(digits);
    Ok(format!("{code:0width$}", width = digits as usize))
}

pub fn totp(input: &str, args: &Value, now: i64) -> Result<Value> {
    let mut secret = input.trim().to_string();
    let mut digits = arg_usize(args, "digits", 6, 6, 8) as u32;
    let mut period = arg_usize(args, "period", 30, 1, 3600) as i64;
    let mut alg = arg_str(args, "algorithm").to_ascii_uppercase();
    let mut label = None;
    let mut issuer = None;
    if secret.starts_with("otpauth://") {
        let u = url::Url::parse(&secret).map_err(|e| invalid(format!("bad otpauth URI: {e}")))?;
        label = Some(percent_encoding::percent_decode_str(u.path().trim_start_matches('/')).decode_utf8_lossy().into_owned());
        let mut s = None;
        for (k, v) in u.query_pairs() {
            match k.as_ref() {
                "secret" => s = Some(v.into_owned()),
                "digits" => digits = v.parse().unwrap_or(digits),
                "period" => period = v.parse().unwrap_or(period),
                "algorithm" => alg = v.to_ascii_uppercase(),
                "issuer" => issuer = Some(v.into_owned()),
                _ => {}
            }
        }
        secret = s.ok_or_else(|| invalid("the otpauth URI has no secret parameter"))?;
    }
    let cleaned: String = secret.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_uppercase();
    let cleaned = cleaned.trim_end_matches('=');
    if cleaned.is_empty() {
        return Err(invalid("paste a Base32 secret (e.g. JBSWY3DPEHPK3PXP) or an otpauth:// URI"));
    }
    let key = data_encoding::BASE32_NOPAD.decode(cleaned.as_bytes()).map_err(|e| invalid(format!("secret is not valid Base32: {e}")))?;
    if !(6..=10).contains(&digits) || period <= 0 {
        return Err(invalid("digits must be 6–10 and period positive"));
    }
    let step = now.div_euclid(period);
    let alg = if matches!(alg.as_str(), "SHA256" | "SHA512") { alg } else { "SHA1".to_string() };
    Ok(json!({
        "code": hotp(&key, step as u64, digits, &alg)?,
        "secondsLeft": period - now.rem_euclid(period),
        "nextCode": hotp(&key, (step + 1) as u64, digits, &alg)?,
        "previousCode": hotp(&key, (step - 1).max(0) as u64, digits, &alg)?,
        "period": period,
        "digits": digits,
        "algorithm": alg,
        "counter": step,
        "label": label,
        "issuer": issuer,
    }))
}

#[cfg(test)]
pub(crate) fn totp_at(secret_b32: &str, now: i64, digits: u32, alg: &str) -> Result<String> {
    let key = data_encoding::BASE32_NOPAD.decode(secret_b32.trim_end_matches('=').as_bytes()).map_err(|e| invalid(e.to_string()))?;
    hotp(&key, (now / 30) as u64, digits, alg)
}

// ── Slug ─────────────────────────────────────────────────────────────────────────────────────────

fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'æ' => "ae",
        'Æ' => "AE",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'Ç' | 'Ć' | 'Č' => "C",
        'ď' | 'đ' | 'ð' => "d",
        'Ď' | 'Đ' | 'Ð' => "D",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ė' | 'Ę' | 'Ě' => "E",
        'ğ' | 'ģ' => "g",
        'Ğ' | 'Ģ' => "G",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'Į' | 'İ' => "I",
        'ķ' => "k",
        'ł' | 'ľ' | 'ļ' | 'ĺ' => "l",
        'Ł' | 'Ľ' | 'Ļ' | 'Ĺ' => "L",
        'ñ' | 'ń' | 'ň' | 'ņ' => "n",
        'Ñ' | 'Ń' | 'Ň' | 'Ņ' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
        'œ' => "oe",
        'Œ' => "OE",
        'ŕ' | 'ř' => "r",
        'Ŕ' | 'Ř' => "R",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'Ś' | 'Š' | 'Ş' | 'Ș' => "S",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'Ť' | 'Ţ' | 'Ț' => "T",
        'þ' => "th",
        'Þ' => "TH",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' | 'Ÿ' => "Y",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        '&' => "and",
        _ => return None,
    })
}

pub fn slug(input: &str, sep: &str, lowercase: bool) -> String {
    let sep = if sep == "_" { '_' } else { '-' };
    let mut folded = String::with_capacity(input.len());
    for c in input.chars() {
        match fold(c) {
            Some(s) if c == '&' => {
                folded.push(' ');
                folded.push_str(s);
                folded.push(' ');
            }
            Some(s) => folded.push_str(s),
            // Combining marks (decomposed accents) vanish.
            None if ('\u{300}'..='\u{36f}').contains(&c) => {}
            None => folded.push(c),
        }
    }
    let mut out = String::new();
    let mut pending = false;
    for c in folded.chars() {
        if c.is_alphanumeric() {
            if pending && !out.is_empty() {
                out.push(sep);
            }
            pending = false;
            if lowercase { out.extend(c.to_lowercase()) } else { out.push(c) }
        } else if c != '\'' && c != '’' {
            pending = true;
        }
    }
    out
}

// ── HTTP status codes ────────────────────────────────────────────────────────────────────────────

const STATUS: &[(u16, &str, &str, &str)] = &[
    (100, "Continue", "The client should continue sending the request body.", "RFC 9110 §15.2.1"),
    (101, "Switching Protocols", "The server is switching protocols (e.g. to WebSocket) as requested via Upgrade.", "RFC 9110 §15.2.2"),
    (102, "Processing", "WebDAV: the request was received and is being processed; no response yet.", "RFC 2518"),
    (103, "Early Hints", "Preliminary headers (usually Link preloads) sent before the final response.", "RFC 8297"),
    (200, "OK", "The request succeeded.", "RFC 9110 §15.3.1"),
    (201, "Created", "A new resource was created; its URI is usually in the Location header.", "RFC 9110 §15.3.2"),
    (202, "Accepted", "Accepted for processing, but processing has not completed (async work).", "RFC 9110 §15.3.3"),
    (203, "Non-Authoritative Information", "The payload was modified by a transforming proxy.", "RFC 9110 §15.3.4"),
    (204, "No Content", "Success with no response body.", "RFC 9110 §15.3.5"),
    (205, "Reset Content", "Success; the client should reset the document view (e.g. clear the form).", "RFC 9110 §15.3.6"),
    (206, "Partial Content", "A range of the resource is returned (Range request).", "RFC 9110 §15.3.7"),
    (207, "Multi-Status", "WebDAV: multiple status codes for multiple sub-requests in an XML body.", "RFC 4918"),
    (208, "Already Reported", "WebDAV: members already enumerated in a previous part of the response.", "RFC 5842"),
    (226, "IM Used", "Delta encoding: the response is the result of instance manipulations.", "RFC 3229"),
    (300, "Multiple Choices", "Several representations are available; the client should choose.", "RFC 9110 §15.4.1"),
    (301, "Moved Permanently", "The resource moved permanently; clients may change POST to GET when following.", "RFC 9110 §15.4.2"),
    (302, "Found", "Temporary redirect; clients may change POST to GET when following.", "RFC 9110 §15.4.3"),
    (303, "See Other", "Redirect to another resource that should be fetched with GET (post/redirect/get).", "RFC 9110 §15.4.4"),
    (304, "Not Modified", "Conditional request: the cached copy is still valid.", "RFC 9110 §15.4.5"),
    (305, "Use Proxy", "Deprecated.", "RFC 9110 §15.4.6"),
    (307, "Temporary Redirect", "Temporary redirect that keeps the method and body.", "RFC 9110 §15.4.8"),
    (308, "Permanent Redirect", "Permanent redirect that keeps the method and body.", "RFC 9110 §15.4.9"),
    (400, "Bad Request", "The request is malformed (syntax, framing, invalid parameters).", "RFC 9110 §15.5.1"),
    (401, "Unauthorized", "Authentication is required or failed; see WWW-Authenticate.", "RFC 9110 §15.5.2"),
    (402, "Payment Required", "Reserved; used ad hoc by some APIs for billing limits.", "RFC 9110 §15.5.3"),
    (403, "Forbidden", "Authenticated (or not) but not allowed to access this resource.", "RFC 9110 §15.5.4"),
    (404, "Not Found", "No resource at this URI (or the server hides its existence).", "RFC 9110 §15.5.5"),
    (405, "Method Not Allowed", "The method is not supported for this resource; see the Allow header.", "RFC 9110 §15.5.6"),
    (406, "Not Acceptable", "No representation matches the Accept headers.", "RFC 9110 §15.5.7"),
    (407, "Proxy Authentication Required", "The client must authenticate with the proxy.", "RFC 9110 §15.5.8"),
    (408, "Request Timeout", "The server timed out waiting for the request.", "RFC 9110 §15.5.9"),
    (409, "Conflict", "The request conflicts with the current state (e.g. edit conflict, duplicate).", "RFC 9110 §15.5.10"),
    (410, "Gone", "The resource is permanently gone with no forwarding address.", "RFC 9110 §15.5.11"),
    (411, "Length Required", "A Content-Length header is required.", "RFC 9110 §15.5.12"),
    (412, "Precondition Failed", "A conditional header (If-Match, If-Unmodified-Since…) evaluated false.", "RFC 9110 §15.5.13"),
    (413, "Content Too Large", "The request body is larger than the server allows.", "RFC 9110 §15.5.14"),
    (414, "URI Too Long", "The request URI is longer than the server will interpret.", "RFC 9110 §15.5.15"),
    (415, "Unsupported Media Type", "The Content-Type / Content-Encoding of the body is not supported.", "RFC 9110 §15.5.16"),
    (416, "Range Not Satisfiable", "The requested range is outside the resource.", "RFC 9110 §15.5.17"),
    (417, "Expectation Failed", "The Expect header can't be met.", "RFC 9110 §15.5.18"),
    (418, "I'm a teapot", "The server refuses to brew coffee because it is a teapot (HTCPCP joke, kept reserved).", "RFC 2324 / RFC 9110 §15.5.19"),
    (421, "Misdirected Request", "The request reached a server that can't answer for this host (e.g. reused TLS connection).", "RFC 9110 §15.5.20"),
    (422, "Unprocessable Content", "Well-formed but semantically invalid (validation errors).", "RFC 9110 §15.5.21"),
    (423, "Locked", "WebDAV: the resource is locked.", "RFC 4918"),
    (424, "Failed Dependency", "WebDAV: a request this one depends on failed.", "RFC 4918"),
    (425, "Too Early", "The server won't process a request that might be replayed (TLS early data).", "RFC 8470"),
    (426, "Upgrade Required", "The client must switch to another protocol (see Upgrade).", "RFC 9110 §15.5.22"),
    (428, "Precondition Required", "The server requires a conditional request (to prevent lost updates).", "RFC 6585"),
    (429, "Too Many Requests", "Rate limited; see Retry-After.", "RFC 6585"),
    (431, "Request Header Fields Too Large", "Headers (one or all) are too large.", "RFC 6585"),
    (451, "Unavailable For Legal Reasons", "Blocked for legal reasons (censorship, court order).", "RFC 7725"),
    (500, "Internal Server Error", "Unexpected server-side failure.", "RFC 9110 §15.6.1"),
    (501, "Not Implemented", "The server doesn't support the functionality (e.g. unknown method).", "RFC 9110 §15.6.2"),
    (502, "Bad Gateway", "A gateway/proxy got an invalid response from upstream.", "RFC 9110 §15.6.3"),
    (503, "Service Unavailable", "Temporarily overloaded or down for maintenance; see Retry-After.", "RFC 9110 §15.6.4"),
    (504, "Gateway Timeout", "A gateway/proxy timed out waiting for upstream.", "RFC 9110 §15.6.5"),
    (505, "HTTP Version Not Supported", "The HTTP version is not supported.", "RFC 9110 §15.6.6"),
    (506, "Variant Also Negotiates", "Content negotiation configuration error (circular reference).", "RFC 2295"),
    (507, "Insufficient Storage", "WebDAV: the server can't store the representation.", "RFC 4918"),
    (508, "Loop Detected", "WebDAV: infinite loop while processing.", "RFC 5842"),
    (510, "Not Extended", "Further extensions are required (obsolete).", "RFC 2774"),
    (511, "Network Authentication Required", "Captive portal: authenticate to gain network access.", "RFC 6585"),
];

fn status_class(code: u16) -> &'static str {
    match code / 100 {
        1 => "Informational",
        2 => "Success",
        3 => "Redirection",
        4 => "Client error",
        5 => "Server error",
        _ => "Unknown",
    }
}

fn status_json(&(code, name, desc, rfc): &(u16, &str, &str, &str)) -> Value {
    json!({ "code": code, "name": name, "class": status_class(code), "description": desc, "reference": rfc })
}

pub fn http_status(input: &str) -> Result<Value> {
    let q = input.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Ok(Value::Array(STATUS.iter().map(status_json).collect()));
    }
    if let Ok(code) = q.parse::<u16>() {
        return match STATUS.iter().find(|s| s.0 == code) {
            Some(s) => Ok(status_json(s)),
            None if (100..600).contains(&code) => Ok(json!({ "code": code, "name": null, "class": status_class(code), "description": "Unassigned code; treat it like the x00 code of its class." })),
            None => Err(invalid("HTTP status codes run from 100 to 599")),
        };
    }
    if q.len() == 3 && q.ends_with("xx") {
        let class: u16 = q[..1].parse().map_err(|_| invalid("use 1xx–5xx"))?;
        return Ok(Value::Array(STATUS.iter().filter(|s| s.0 / 100 == class).map(status_json).collect()));
    }
    let hits: Vec<Value> = STATUS.iter().filter(|s| s.1.to_ascii_lowercase().contains(&q) || s.2.to_ascii_lowercase().contains(&q)).map(status_json).collect();
    if hits.is_empty() {
        return Err(invalid(format!("no status code matches {input:?}")));
    }
    Ok(Value::Array(hits))
}

// ── User agent ───────────────────────────────────────────────────────────────────────────────────

pub fn user_agent(input: &str) -> Value {
    let unknown = |s: &str| if s == "UNKNOWN" || s.is_empty() { Value::Null } else { json!(s) };
    match woothee::parser::Parser::new().parse(input) {
        Some(r) => {
            let device = match r.category {
                "smartphone" => "mobile",
                "mobilephone" => "feature phone",
                "pc" => "desktop",
                "crawler" => "bot",
                "appliance" => "appliance",
                _ => "unknown",
            };
            let engine = if input.contains("Gecko/") && input.contains("Firefox") {
                "Gecko"
            } else if input.contains("AppleWebKit") && (input.contains("Chrome/") || input.contains("Chromium") || input.contains("Edg/")) {
                "Blink"
            } else if input.contains("AppleWebKit") {
                "WebKit"
            } else if input.contains("Trident") {
                "Trident"
            } else {
                ""
            };
            json!({
                "browser": unknown(r.name),
                "version": unknown(r.version),
                "engine": unknown(engine),
                "os": unknown(r.os),
                "osVersion": unknown(&r.os_version),
                "device": device,
                "category": unknown(r.category),
                "vendor": unknown(r.vendor),
                "isBot": r.category == "crawler",
                "isMobile": matches!(r.category, "smartphone" | "mobilephone"),
            })
        }
        None => json!({ "browser": null, "os": null, "device": "unknown", "isBot": false, "isMobile": false }),
    }
}

// ── chmod ────────────────────────────────────────────────────────────────────────────────────────

fn rwx(bits: u32, special: bool, special_char: char) -> String {
    let r = if bits & 4 != 0 { 'r' } else { '-' };
    let w = if bits & 2 != 0 { 'w' } else { '-' };
    let x = match (bits & 1 != 0, special) {
        (true, true) => special_char,
        (false, true) => special_char.to_ascii_uppercase(),
        (true, false) => 'x',
        (false, false) => '-',
    };
    format!("{r}{w}{x}")
}

fn parse_symbolic_rwx(s: &str) -> Result<u32> {
    // 9 chars rwxrwxrwx (optionally with s/S/t/T).
    let c: Vec<char> = s.chars().collect();
    if c.len() != 9 {
        return Err(invalid("expected 9 permission characters like rwxr-xr-x"));
    }
    let mut mode = 0u32;
    for (i, ch) in c.iter().enumerate() {
        let (who, pos) = (i / 3, i % 3);
        let shift = (2 - who) * 3;
        match (pos, ch) {
            (0, 'r') => mode |= 4 << shift,
            (1, 'w') => mode |= 2 << shift,
            (2, 'x') => mode |= 1 << shift,
            (2, 's') if who < 2 => mode |= (1 << shift) | if who == 0 { 0o4000 } else { 0o2000 },
            (2, 'S') if who < 2 => mode |= if who == 0 { 0o4000 } else { 0o2000 },
            (2, 't') if who == 2 => mode |= 1 | 0o1000,
            (2, 'T') if who == 2 => mode |= 0o1000,
            (_, '-') => {}
            _ => return Err(invalid(format!("unexpected '{ch}' at position {}", i + 1))),
        }
    }
    Ok(mode)
}

fn parse_clauses(s: &str) -> Result<u32> {
    // u=rwx,g=rx,o= / a+x / ug=rw (applied to 000)
    let mut mode = 0u32;
    for clause in s.split(',') {
        let op_at = clause.find(['=', '+', '-']).ok_or_else(|| invalid(format!("bad clause {clause:?} (use u=rwx,g=rx,o=r)")))?;
        let (who, rest) = clause.split_at(op_at);
        let op = rest.chars().next().unwrap_or('=');
        let perms = &rest[1..];
        let whos: Vec<u32> = if who.is_empty() || who.contains('a') {
            vec![6, 3, 0]
        } else {
            who.chars()
                .map(|w| match w {
                    'u' => Ok(6),
                    'g' => Ok(3),
                    'o' => Ok(0),
                    _ => Err(invalid(format!("unknown class '{w}' (u, g, o, a)"))),
                })
                .collect::<Result<_>>()?
        };
        for shift in whos {
            let mut bits = 0u32;
            let mut special = 0u32;
            for p in perms.chars() {
                match p {
                    'r' => bits |= 4,
                    'w' => bits |= 2,
                    'x' => bits |= 1,
                    's' if shift == 6 => special |= 0o4000,
                    's' if shift == 3 => special |= 0o2000,
                    't' => special |= 0o1000,
                    's' => {}
                    _ => return Err(invalid(format!("unknown permission '{p}'"))),
                }
            }
            match op {
                '=' => {
                    mode &= !(7 << shift);
                    mode |= bits << shift | special;
                }
                '+' => mode |= bits << shift | special,
                _ => mode &= !(bits << shift | special),
            }
        }
    }
    Ok(mode)
}

pub fn chmod(input: &str) -> Result<Value> {
    let s = input.trim();
    if s.is_empty() {
        return Err(invalid("enter 755, rwxr-xr-x, -rwxr-xr-x or u=rwx,g=rx,o=rx"));
    }
    let mode = if s.chars().all(|c| c.is_digit(8)) && (1..=4).contains(&s.len()) {
        u32::from_str_radix(s, 8).map_err(|e| invalid(e.to_string()))?
    } else if s.contains(['=', '+']) || (s.contains('-') && s.chars().next().is_some_and(|c| "ugoa".contains(c))) {
        parse_clauses(s)?
    } else if s.len() == 10 {
        parse_symbolic_rwx(&s[1..])?
    } else {
        parse_symbolic_rwx(s)?
    };
    let (u, g, o) = ((mode >> 6) & 7, (mode >> 3) & 7, mode & 7);
    let symbolic = format!("{}{}{}", rwx(u, mode & 0o4000 != 0, 's'), rwx(g, mode & 0o2000 != 0, 's'), rwx(o, mode & 0o1000 != 0, 't'));
    let who = |b: u32| json!({ "read": b & 4 != 0, "write": b & 2 != 0, "execute": b & 1 != 0 });
    let octal = if mode > 0o777 { format!("{mode:04o}") } else { format!("{mode:03o}") };
    let mut warnings = Vec::new();
    if o & 2 != 0 {
        warnings.push("World-writable".to_string());
    }
    if mode & 0o4000 != 0 {
        warnings.push("setuid: runs with the owner's privileges".to_string());
    }
    Ok(json!({
        "octal": octal,
        "symbolic": symbolic,
        "ls": format!("-{symbolic}"),
        "owner": who(u),
        "group": who(g),
        "others": who(o),
        "setuid": mode & 0o4000 != 0,
        "setgid": mode & 0o2000 != 0,
        "sticky": mode & 0o1000 != 0,
        "command": format!("chmod {octal} <file>"),
        "symbolicCommand": format!("chmod u={},g={},o={} <file>", rwx(u, false, 'x').replace('-', ""), rwx(g, false, 'x').replace('-', ""), rwx(o, false, 'x').replace('-', "")),
        "warnings": warnings,
    }))
}
