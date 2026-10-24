//! NTLM authentication (MS-NLMP) with NTLMv2 responses: the NEGOTIATE message (type 1), parsing
//! the server's CHALLENGE (type 2) and the AUTHENTICATE answer (type 3). NTLM authenticates the
//! *connection*, so [`crate::ClientPool`] sends the legs over a dedicated HTTP/1.1 client that keeps
//! a single connection alive. MD4 (only needed for the NT hash) is implemented here.
//!
//! Not implemented: NTLMv1/LM responses (insecure, disabled on modern Windows), session security
//! (signing/sealing — HTTP doesn't use it) and the optional MIC.

use base64::Engine;
use hmac::{KeyInit, Mac};

type HmacMd5 = hmac::Hmac<md5::Md5>;

const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";
const NEGOTIATE_UNICODE: u32 = 0x0000_0001;
const NEGOTIATE_OEM: u32 = 0x0000_0002;
const REQUEST_TARGET: u32 = 0x0000_0004;
const NEGOTIATE_NTLM: u32 = 0x0000_0200;
const NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
const NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
const NEGOTIATE_TARGET_INFO: u32 = 0x0080_0000;
const NEGOTIATE_128: u32 = 0x2000_0000;
const NEGOTIATE_56: u32 = 0x8000_0000;

/// Flags offered in the NEGOTIATE message.
const CLIENT_FLAGS: u32 = NEGOTIATE_UNICODE
    | NEGOTIATE_OEM
    | REQUEST_TARGET
    | NEGOTIATE_NTLM
    | NEGOTIATE_ALWAYS_SIGN
    | NEGOTIATE_EXTENDED_SESSIONSECURITY
    | NEGOTIATE_TARGET_INFO
    | NEGOTIATE_128
    | NEGOTIATE_56;

const AV_EOL: u16 = 0;
const AV_TIMESTAMP: u16 = 7;

/// Resolved NTLM credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub domain: String,
    pub workstation: String,
}

impl Credentials {
    /// `DOMAIN\user` and `user@domain` are split when no explicit domain is given.
    pub fn new(username: &str, password: &str, domain: Option<&str>, workstation: Option<&str>) -> Self {
        let domain = domain.map(str::trim).filter(|d| !d.is_empty());
        let (user, dom) = match domain {
            Some(d) => (username.trim().to_string(), d.to_string()),
            None => match username.trim().split_once('\\') {
                Some((d, u)) => (u.to_string(), d.to_string()),
                None => match username.trim().rsplit_once('@') {
                    Some((u, d)) => (u.to_string(), d.to_string()),
                    None => (username.trim().to_string(), String::new()),
                },
            },
        };
        Self { username: user, password: password.to_string(), domain: dom, workstation: workstation.unwrap_or_default().trim().to_string() }
    }

    /// `DOMAIN\user` (or just `user`), as curl's `-u` expects it.
    pub fn qualified_user(&self) -> String {
        if self.domain.is_empty() { self.username.clone() } else { format!("{}\\{}", self.domain, self.username) }
    }
}

/// MD4 (RFC 1320).
pub fn md4(input: &[u8]) -> [u8; 16] {
    let mut msg = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    let mut s = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
    let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
    let h = |x: u32, y: u32, z: u32| x ^ y ^ z;
    for block in msg.chunks_exact(64) {
        let x: Vec<u32> = block.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let saved = s;
        // Step i updates a, d, c, b in turn; the other three are the round function's inputs.
        let mut step = |i: usize, k: usize, shift: u32, round: &dyn Fn(u32, u32, u32) -> u32, add: u32| {
            let t = (4 - i % 4) % 4;
            let v = s[t].wrapping_add(round(s[(t + 1) % 4], s[(t + 2) % 4], s[(t + 3) % 4])).wrapping_add(x[k]).wrapping_add(add);
            s[t] = v.rotate_left(shift);
        };
        for i in 0..16 {
            step(i, i, [3, 7, 11, 19][i % 4], &f, 0);
        }
        for i in 0..16 {
            step(i, [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15][i], [3, 5, 9, 13][i % 4], &g, 0x5a82_7999);
        }
        for i in 0..16 {
            step(i, [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15][i], [3, 9, 11, 15][i % 4], &h, 0x6ed9_eba1);
        }
        for (v, old) in s.iter_mut().zip(saved) {
            *v = v.wrapping_add(old);
        }
    }
    let mut out = [0u8; 16];
    for (i, v) in s.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let mut m = HmacMd5::new_from_slice(key).expect("HMAC accepts any key length");
    for p in parts {
        m.update(p);
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&m.finalize().into_bytes());
    out
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// NTOWFv2 (= LMOWFv2): HMAC-MD5(MD4(UTF-16LE(password)), UTF-16LE(UPPER(user) + domain)).
pub fn nt_owf_v2(username: &str, password: &str, domain: &str) -> [u8; 16] {
    let nt_hash = md4(&utf16le(password));
    hmac_md5(&nt_hash, &[&utf16le(&format!("{}{domain}", username.to_uppercase()))])
}

/// The NEGOTIATE (type 1) message.
pub fn negotiate_message() -> Vec<u8> {
    let mut m = Vec::with_capacity(32);
    m.extend_from_slice(SIGNATURE);
    m.extend_from_slice(&1u32.to_le_bytes());
    m.extend_from_slice(&CLIENT_FLAGS.to_le_bytes());
    // Empty domain and workstation fields (len, max len, offset).
    for _ in 0..2 {
        m.extend_from_slice(&0u16.to_le_bytes());
        m.extend_from_slice(&0u16.to_le_bytes());
        m.extend_from_slice(&32u32.to_le_bytes());
    }
    m
}

/// The server's CHALLENGE (type 2) message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    pub target_name: String,
    /// Raw AV pairs (`TargetInfo`), echoed in the NTLMv2 response.
    pub target_info: Vec<u8>,
    /// `MsvAvTimestamp`, when the server sent one.
    pub timestamp: Option<[u8; 8]>,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// The bytes a security buffer (len, max len, offset) at `at` points to.
fn field(b: &[u8], at: usize) -> Option<&[u8]> {
    let len = u16_at(b, at)? as usize;
    let off = u32_at(b, at + 4)? as usize;
    if len == 0 {
        return Some(&[]);
    }
    b.get(off..off.checked_add(len)?)
}

pub fn parse_challenge(msg: &[u8]) -> Result<Challenge, String> {
    let bad = || "malformed NTLM challenge from the server".to_string();
    if msg.len() < 32 || &msg[..8] != SIGNATURE {
        return Err(bad());
    }
    if u32_at(msg, 8) != Some(2) {
        return Err("the server sent an NTLM message that is not a challenge".into());
    }
    let flags = u32_at(msg, 20).ok_or_else(bad)?;
    let server_challenge: [u8; 8] = msg[24..32].try_into().map_err(|_| bad())?;
    let unicode = flags & NEGOTIATE_UNICODE != 0;
    let name = field(msg, 12).ok_or_else(bad)?;
    let target_name = if unicode {
        String::from_utf16_lossy(&name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        String::from_utf8_lossy(name).into_owned()
    };
    let target_info = if msg.len() >= 48 { field(msg, 40).ok_or_else(bad)?.to_vec() } else { Vec::new() };
    let mut timestamp = None;
    let mut i = 0;
    while let (Some(id), Some(len)) = (u16_at(&target_info, i), u16_at(&target_info, i + 2)) {
        if id == AV_EOL {
            break;
        }
        let value = target_info.get(i + 4..i + 4 + len as usize).ok_or_else(bad)?;
        if id == AV_TIMESTAMP && len == 8 {
            timestamp = value.try_into().ok();
        }
        i += 4 + len as usize;
    }
    Ok(Challenge { flags, server_challenge, target_name, target_info, timestamp })
}

/// NTLMv2 and LMv2 responses plus the session base key (MS-NLMP 3.3.2).
pub struct V2Responses {
    pub nt_response: Vec<u8>,
    pub lm_response: Vec<u8>,
    pub session_base_key: [u8; 16],
}

pub fn v2_responses(creds: &Credentials, ch: &Challenge, client_challenge: [u8; 8], time: [u8; 8]) -> V2Responses {
    let key = nt_owf_v2(&creds.username, &creds.password, &creds.domain);
    let mut temp = vec![1u8, 1, 0, 0, 0, 0, 0, 0];
    temp.extend_from_slice(&time);
    temp.extend_from_slice(&client_challenge);
    temp.extend_from_slice(&[0; 4]);
    temp.extend_from_slice(&ch.target_info);
    temp.extend_from_slice(&[0; 4]);
    let nt_proof = hmac_md5(&key, &[&ch.server_challenge, &temp]);
    let mut nt_response = nt_proof.to_vec();
    nt_response.extend_from_slice(&temp);
    // With a server timestamp the LMv2 response must be all zeroes (MS-NLMP 3.1.5.1.2).
    let lm_response = if ch.timestamp.is_some() {
        vec![0; 24]
    } else {
        let mut lm = hmac_md5(&key, &[&ch.server_challenge, &client_challenge]).to_vec();
        lm.extend_from_slice(&client_challenge);
        lm
    };
    V2Responses { nt_response, lm_response, session_base_key: hmac_md5(&key, &[&nt_proof]) }
}

/// Current time as a Windows FILETIME (100 ns ticks since 1601-01-01), little-endian.
pub fn filetime_now() -> [u8; 8] {
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let ticks = (unix.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(unix.subsec_nanos() / 100);
    ticks.to_le_bytes()
}

/// The AUTHENTICATE (type 3) message answering `ch`.
pub fn authenticate_message(creds: &Credentials, ch: &Challenge, client_challenge: [u8; 8], time: [u8; 8]) -> Vec<u8> {
    let time = ch.timestamp.unwrap_or(time);
    let r = v2_responses(creds, ch, client_challenge, time);
    let unicode = ch.flags & NEGOTIATE_UNICODE != 0;
    let enc = |s: &str| if unicode { utf16le(s) } else { s.as_bytes().to_vec() };
    let flags = (CLIENT_FLAGS & ch.flags) | NEGOTIATE_NTLM | if unicode { NEGOTIATE_UNICODE } else { NEGOTIATE_OEM };
    // Payload order: domain, user, workstation, LM, NT (session key empty: no key exchange).
    let payload: [Vec<u8>; 6] = [enc(&creds.domain), enc(&creds.username), enc(&creds.workstation), r.lm_response, r.nt_response, Vec::new()];
    const HEADER: usize = 64;
    let mut offsets = [0usize; 6];
    let mut at = HEADER;
    for (i, p) in payload.iter().enumerate() {
        offsets[i] = at;
        at += p.len();
    }
    let mut m = Vec::with_capacity(at);
    m.extend_from_slice(SIGNATURE);
    m.extend_from_slice(&3u32.to_le_bytes());
    // Security buffers in header order: LM, NT, domain, user, workstation, session key.
    for i in [3, 4, 0, 1, 2, 5] {
        let len = payload[i].len() as u16;
        m.extend_from_slice(&len.to_le_bytes());
        m.extend_from_slice(&len.to_le_bytes());
        m.extend_from_slice(&(offsets[i] as u32).to_le_bytes());
    }
    m.extend_from_slice(&flags.to_le_bytes());
    for p in &payload {
        m.extend_from_slice(p);
    }
    m
}

/// Finds an NTLM challenge in `WWW-Authenticate` values: `(scheme, decoded message)`, where the
/// scheme is `NTLM` or `Negotiate` (servers offering only Negotiate accept raw NTLM tokens).
pub fn find_challenge<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<(&'static str, Vec<u8>)> {
    for v in values {
        for part in v.split(',') {
            let part = part.trim();
            let (scheme, token) = part.split_once(' ').unwrap_or((part, ""));
            let scheme = if scheme.eq_ignore_ascii_case("ntlm") {
                "NTLM"
            } else if scheme.eq_ignore_ascii_case("negotiate") {
                "Negotiate"
            } else {
                continue;
            };
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(token.trim())
                && bytes.starts_with(SIGNATURE)
            {
                return Some((scheme, bytes));
            }
        }
    }
    None
}

/// `NTLM <base64>` header value.
pub fn header(scheme: &str, msg: &[u8]) -> String {
    format!("{scheme} {}", base64::engine::general_purpose::STANDARD.encode(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(bytes: &[u8]) -> String {
        hex::encode(bytes)
    }

    #[test]
    fn md4_rfc1320_vectors() {
        for (input, want) in [
            ("", "31d6cfe0d16ae931b73c59d7e0c089c0"),
            ("a", "bde52cb31de33e46245e05fbdbd6fb24"),
            ("abc", "a448017aaf21d8525fc10ae87aa6729d"),
            ("message digest", "d9130a8164549fe818874806e1c7014b"),
            ("abcdefghijklmnopqrstuvwxyz", "d79e1c308aa5bbcdeea8ed63df412da9"),
            ("12345678901234567890123456789012345678901234567890123456789012345678901234567890", "e33b4ddc9c38f2199c3e7b164fcc0536"),
        ] {
            assert_eq!(h(&md4(input.as_bytes())), want, "MD4({input:?})");
        }
    }

    /// MS-NLMP 4.2.1 common values + 4.2.4 NTLMv2 authentication.
    fn spec() -> (Credentials, Challenge) {
        let creds = Credentials::new("User", "Password", Some("Domain"), Some("COMPUTER"));
        // MsvAvNbDomainName "Domain", MsvAvNbComputerName "Server", MsvAvEOL.
        let target_info = hex::decode("02000c0044006f006d00610069006e0001000c0053006500720076006500720000000000").unwrap();
        let ch = Challenge {
            flags: 0xe28a_8233,
            server_challenge: [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            target_name: "Domain".into(),
            target_info,
            timestamp: None,
        };
        (creds, ch)
    }

    #[test]
    fn ms_nlmp_ntlmv2_vectors() {
        // 4.2.2.1.2 NTOWFv1 = MD4(UNICODE("Password")).
        assert_eq!(h(&md4(&utf16le("Password"))), "a4f49c406510bdcab6824ee7c30fd852");
        // 4.2.4.1.1 NTOWFv2 / LMOWFv2.
        assert_eq!(h(&nt_owf_v2("User", "Password", "Domain")), "0c868a403bfd7a93a3001ef22ef02e3f");
        let (creds, ch) = spec();
        let r = v2_responses(&creds, &ch, [0xaa; 8], [0; 8]);
        // 4.2.4.2.1 LMv2 response, 4.2.4.2.2 NTProofStr, 4.2.4.1.2 session base key.
        assert_eq!(h(&r.lm_response), "86c35097ac9cec102554764a57cccc19aaaaaaaaaaaaaaaa");
        assert_eq!(h(&r.nt_response[..16]), "68cd0ab851e51c96aabc927bebef6a1c");
        assert_eq!(h(&r.session_base_key), "8de40ccadbc14a82f15cb0ad0de95ca3");
    }

    #[test]
    fn messages_round_trip() {
        let t1 = negotiate_message();
        assert_eq!(&t1[..8], SIGNATURE);
        assert_eq!(u32_at(&t1, 8), Some(1));

        // A challenge as a server would send it (target name + target info in the payload).
        let (creds, spec_ch) = spec();
        let name = utf16le("Domain");
        let mut c = Vec::new();
        c.extend_from_slice(SIGNATURE);
        c.extend_from_slice(&2u32.to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&(name.len() as u16).to_le_bytes());
        c.extend_from_slice(&48u32.to_le_bytes());
        c.extend_from_slice(&spec_ch.flags.to_le_bytes());
        c.extend_from_slice(&spec_ch.server_challenge);
        c.extend_from_slice(&[0; 8]);
        c.extend_from_slice(&(spec_ch.target_info.len() as u16).to_le_bytes());
        c.extend_from_slice(&(spec_ch.target_info.len() as u16).to_le_bytes());
        c.extend_from_slice(&((48 + name.len()) as u32).to_le_bytes());
        c.extend_from_slice(&name);
        c.extend_from_slice(&spec_ch.target_info);
        let parsed = parse_challenge(&c).unwrap();
        assert_eq!(parsed, spec_ch);
        assert!(parse_challenge(&t1).is_err(), "a negotiate message is not a challenge");
        assert!(parse_challenge(b"garbage").is_err());

        let header_value = header("NTLM", &c);
        let (scheme, decoded) = find_challenge(["Basic realm=\"x\"", header_value.as_str()]).unwrap();
        assert_eq!((scheme, decoded.as_slice()), ("NTLM", c.as_slice()));
        assert!(find_challenge(["NTLM", "Negotiate"]).is_none(), "bare scheme offers carry no challenge");

        let t3 = authenticate_message(&creds, &parsed, [0xaa; 8], [0; 8]);
        assert_eq!(u32_at(&t3, 8), Some(3));
        assert_eq!(field(&t3, 28).unwrap(), utf16le("Domain").as_slice());
        assert_eq!(field(&t3, 36).unwrap(), utf16le("User").as_slice());
        assert_eq!(field(&t3, 44).unwrap(), utf16le("COMPUTER").as_slice());
        assert_eq!(h(&field(&t3, 20).unwrap()[..16]), "68cd0ab851e51c96aabc927bebef6a1c");
        assert_eq!(field(&t3, 12).unwrap().len(), 24);
        assert_eq!(field(&t3, 52).unwrap().len(), 0);
    }

    #[test]
    fn server_timestamp_wins_and_zeroes_lm() {
        let (creds, mut ch) = spec();
        let mut info = hex::decode("0700080001020304050607080000").unwrap();
        info.splice(0..0, ch.target_info[..ch.target_info.len() - 4].iter().copied());
        ch.target_info = info;
        ch.timestamp = Some([1, 2, 3, 4, 5, 6, 7, 8]);
        let t3 = authenticate_message(&creds, &ch, [0xaa; 8], [9; 8]);
        assert_eq!(field(&t3, 12).unwrap(), &[0u8; 24]);
        let nt = field(&t3, 20).unwrap();
        assert_eq!(&nt[24..32], &[1, 2, 3, 4, 5, 6, 7, 8], "the server's timestamp is used");
    }

    #[test]
    fn user_forms() {
        assert_eq!(Credentials::new("CORP\\ada", "p", None, None).domain, "CORP");
        assert_eq!(Credentials::new("ada@corp.example", "p", None, None).domain, "corp.example");
        let c = Credentials::new("CORP\\ada", "p", Some("OTHER"), None);
        assert_eq!((c.username.as_str(), c.domain.as_str()), ("CORP\\ada", "OTHER"));
        assert_eq!(Credentials::new("ada", "p", Some("CORP"), None).qualified_user(), "CORP\\ada");
    }
}
