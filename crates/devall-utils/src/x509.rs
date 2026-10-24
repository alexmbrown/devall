//! X.509 certificate decoding (PEM, Base64 DER or hex DER; one cert or a whole chain).

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use x509_parser::objects::{oid2sn, oid_registry};
use x509_parser::oid_registry::Oid;
use x509_parser::prelude::*;

use crate::{Result, decode_base64, invalid};

/// Days before expiry that trigger an "expires soon" warning.
const EXPIRY_WARNING_DAYS: i64 = 30;

fn colon_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn oid_name(oid: &Oid) -> String {
    oid2sn(oid, oid_registry()).map(str::to_string).unwrap_or_else(|_| oid.to_id_string())
}

fn curve_name(oid: &str) -> &'static str {
    match oid {
        "1.2.840.10045.3.1.7" => "P-256",
        "1.3.132.0.34" => "P-384",
        "1.3.132.0.35" => "P-521",
        "1.3.132.0.10" => "secp256k1",
        _ => "",
    }
}

/// Splits the input into DER blobs: PEM blocks if present, else Base64 or hex DER.
pub(crate) fn ders(input: &str) -> Result<Vec<Vec<u8>>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(invalid("paste a PEM certificate (-----BEGIN CERTIFICATE-----) or Base64 DER"));
    }
    if trimmed.contains("-----BEGIN") {
        let mut out = Vec::new();
        for pem in Pem::iter_from_buffer(trimmed.as_bytes()) {
            let pem = pem.map_err(|e| invalid(format!("bad PEM block: {e}")))?;
            if pem.label.contains("CERTIFICATE") {
                out.push(pem.contents);
            }
        }
        if out.is_empty() {
            return Err(invalid("no CERTIFICATE block found (private keys and CSRs are not decoded here)"));
        }
        return Ok(out);
    }
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace() && *c != ':').collect();
    let der = if compact.len().is_multiple_of(2) && compact.chars().all(|c| c.is_ascii_hexdigit()) {
        hex::decode(&compact).map_err(|e| invalid(e.to_string()))?
    } else {
        decode_base64(&compact)?
    };
    // A DER blob may hold several concatenated certificates.
    let mut out = Vec::new();
    let mut rest: &[u8] = &der;
    while !rest.is_empty() {
        match X509Certificate::from_der(rest) {
            Ok((next, _)) => {
                out.push(rest[..rest.len() - next.len()].to_vec());
                rest = next;
            }
            Err(e) if out.is_empty() => return Err(invalid(format!("not a DER certificate: {e}"))),
            Err(_) => break,
        }
    }
    Ok(out)
}

fn general_name(g: &GeneralName) -> String {
    match g {
        GeneralName::DNSName(s) => format!("DNS:{s}"),
        GeneralName::RFC822Name(s) => format!("email:{s}"),
        GeneralName::URI(s) => format!("URI:{s}"),
        GeneralName::IPAddress(b) => match b.len() {
            4 => format!("IP:{}", std::net::Ipv4Addr::from(<[u8; 4]>::try_from(*b).unwrap_or_default())),
            16 => format!("IP:{}", std::net::Ipv6Addr::from(<[u8; 16]>::try_from(*b).unwrap_or_default())),
            _ => format!("IP:{}", hex::encode(b)),
        },
        GeneralName::DirectoryName(n) => format!("DirName:{n}"),
        GeneralName::RegisteredID(o) => format!("RID:{}", o.to_id_string()),
        GeneralName::OtherName(o, _) => format!("othername:{}", o.to_id_string()),
        other => format!("{other:?}"),
    }
}

fn name_fields(n: &X509Name) -> Value {
    let first = |it: &mut dyn Iterator<Item = &AttributeTypeAndValue>| it.next().and_then(|a| a.as_str().ok()).map(str::to_string);
    json!({
        "commonName": first(&mut n.iter_common_name()),
        "organization": first(&mut n.iter_organization()),
        "organizationalUnit": first(&mut n.iter_organizational_unit()),
        "country": first(&mut n.iter_country()),
        "state": first(&mut n.iter_state_or_province()),
        "locality": first(&mut n.iter_locality()),
    })
}

fn iso(ts: i64) -> Option<String> {
    Utc.timestamp_opt(ts, 0).single().map(|d| d.to_rfc3339())
}

/// Summary of one certificate. `now` is unix seconds (for expiry status).
pub(crate) fn summarize(der: &[u8], now: i64) -> Result<Value> {
    let (_, cert) = X509Certificate::from_der(der).map_err(|e| invalid(format!("cannot parse certificate: {e}")))?;
    let mut warnings: Vec<String> = Vec::new();
    let not_before = cert.validity().not_before.timestamp();
    let not_after = cert.validity().not_after.timestamp();
    let days_left = (not_after - now).div_euclid(86_400);
    let status = if now > not_after {
        warnings.push(format!("EXPIRED {} day(s) ago", (now - not_after) / 86_400));
        "expired"
    } else if now < not_before {
        warnings.push("Not valid yet".into());
        "not-yet-valid"
    } else {
        if days_left < EXPIRY_WARNING_DAYS {
            warnings.push(format!("Expires in {days_left} day(s)"));
        }
        "valid"
    };

    let spki = cert.public_key();
    let key_alg = oid_name(&spki.algorithm.algorithm);
    let (key_bits, curve) = match spki.parsed() {
        Ok(k) => {
            let curve = spki.algorithm.parameters.as_ref().and_then(|p| p.as_oid().ok()).map(|o| curve_name(&o.to_id_string()).to_string()).filter(|c| !c.is_empty());
            (Some(k.key_size()).filter(|b| *b > 0), curve)
        }
        Err(_) => (None, None),
    };
    if key_alg.contains("rsa") && key_bits.is_some_and(|b| b < 2048) {
        warnings.push(format!("Weak RSA key ({} bits)", key_bits.unwrap_or(0)));
    }
    let sig_alg = oid_name(&cert.signature_algorithm.algorithm);
    if sig_alg.to_ascii_lowercase().contains("sha1") || sig_alg.to_ascii_lowercase().contains("md5") {
        warnings.push(format!("Weak signature algorithm ({sig_alg})"));
    }

    let sans: Vec<String> = cert.subject_alternative_name().ok().flatten().map(|e| e.value.general_names.iter().map(general_name).collect()).unwrap_or_default();
    let self_signed = cert.subject().as_raw() == cert.issuer().as_raw();
    let (is_ca, path_len) = match cert.basic_constraints() {
        Ok(Some(bc)) => (bc.value.ca, bc.value.path_len_constraint),
        _ => (false, None),
    };
    if sans.is_empty() && !is_ca {
        warnings.push("No subjectAltName: browsers ignore the CN for host names".into());
    }
    if self_signed && !is_ca {
        warnings.push("Self-signed".into());
    }
    let key_usage = cert.key_usage().ok().flatten().map(|k| k.value.to_string().split(", ").filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>());
    let eku = cert.extended_key_usage().ok().flatten().map(|e| {
        let v = e.value;
        let mut out: Vec<String> = [
            (v.server_auth, "serverAuth"),
            (v.client_auth, "clientAuth"),
            (v.code_signing, "codeSigning"),
            (v.email_protection, "emailProtection"),
            (v.time_stamping, "timeStamping"),
            (v.ocsp_signing, "OCSPSigning"),
            (v.any, "anyExtendedKeyUsage"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| (*n).to_string())
        .collect();
        out.extend(v.other.iter().map(|o| o.to_id_string()));
        out
    });
    let mut ski = None;
    let mut aki = None;
    let mut aia: Vec<Value> = Vec::new();
    let mut crl: Vec<String> = Vec::new();
    let extensions: Vec<Value> = cert
        .extensions()
        .iter()
        .map(|ext| {
            match ext.parsed_extension() {
                ParsedExtension::SubjectKeyIdentifier(k) => ski = Some(colon_hex(k.0)),
                ParsedExtension::AuthorityKeyIdentifier(a) => aki = a.key_identifier.as_ref().map(|k| colon_hex(k.0)),
                ParsedExtension::AuthorityInfoAccess(a) => {
                    aia.extend(a.accessdescs.iter().map(|d| json!({ "method": oid_name(&d.access_method), "location": general_name(&d.access_location) })))
                }
                ParsedExtension::CRLDistributionPoints(points) => {
                    for p in points.iter() {
                        if let Some(DistributionPointName::FullName(names)) = &p.distribution_point {
                            crl.extend(names.iter().map(general_name));
                        }
                    }
                }
                _ => {}
            }
            json!({ "oid": ext.oid.to_id_string(), "name": oid_name(&ext.oid), "critical": ext.critical })
        })
        .collect();

    Ok(json!({
        "subject": cert.subject().to_string(),
        "subjectFields": name_fields(cert.subject()),
        "issuer": cert.issuer().to_string(),
        "issuerFields": name_fields(cert.issuer()),
        "serial": colon_hex(cert.raw_serial()),
        "version": cert.version().0 + 1,
        "validity": {
            "notBefore": iso(not_before),
            "notAfter": iso(not_after),
            "daysRemaining": days_left,
            "status": status,
        },
        "subjectAltNames": sans,
        "publicKey": { "algorithm": key_alg, "bits": key_bits, "curve": curve },
        "signatureAlgorithm": sig_alg,
        "fingerprints": {
            "sha256": colon_hex(&Sha256::digest(der)),
            "sha1": colon_hex(&Sha1::digest(der)),
        },
        "isCa": is_ca,
        "pathLength": path_len,
        "selfSigned": self_signed,
        "keyUsage": key_usage,
        "extendedKeyUsage": eku,
        "subjectKeyId": ski,
        "authorityKeyId": aki,
        "authorityInfoAccess": aia,
        "crlDistributionPoints": crl,
        "extensions": extensions,
        "warnings": warnings,
    }))
}

/// Chain linkage checks: each certificate should be issued by the next one.
pub(crate) fn chain_notes(certs: &[Value]) -> Vec<String> {
    let mut notes = Vec::new();
    for (i, pair) in certs.windows(2).enumerate() {
        if pair[0]["issuer"] != pair[1]["subject"] {
            notes.push(format!("Certificate #{} was not issued by #{} (chain out of order or an intermediate is missing)", i + 1, i + 2));
        }
    }
    if let Some(last) = certs.last()
        && certs.len() > 1
        && last["selfSigned"] == true
    {
        notes.push("The chain includes the self-signed root; servers normally omit it".into());
    }
    notes
}

pub fn decode(input: &str, now: i64) -> Result<Value> {
    let certs: Vec<Value> = ders(input)?.iter().map(|d| summarize(d, now)).collect::<Result<_>>()?;
    let notes = chain_notes(&certs);
    Ok(json!({ "count": certs.len(), "certificates": certs, "chainWarnings": notes }))
}
