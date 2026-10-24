//! Network helpers: CIDR maths, IP classification (pure), plus two user-triggered network ops:
//! DNS lookups through the system resolver config and TLS certificate-chain inspection.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use serde_json::{Value, json};

use crate::{Result, invalid, x509};

const NET_TIMEOUT: Duration = Duration::from_secs(8);

// ── IP classification ────────────────────────────────────────────────────────────────────────────

fn in4(ip: Ipv4Addr, net: &str) -> bool {
    net.parse::<Ipv4Net>().is_ok_and(|n| n.contains(&ip))
}

fn in6(ip: Ipv6Addr, net: &str) -> bool {
    net.parse::<Ipv6Net>().is_ok_and(|n| n.contains(&ip))
}

/// (tags, scope) for an address; scope is the single most useful label.
fn classify(ip: IpAddr) -> (Vec<&'static str>, &'static str) {
    let mut tags: Vec<&'static str> = Vec::new();
    match ip {
        IpAddr::V4(v) => {
            let table: &[(&str, &str)] = &[
                ("0.0.0.0/32", "unspecified"),
                ("0.0.0.0/8", "this-network"),
                ("127.0.0.0/8", "loopback"),
                ("10.0.0.0/8", "private"),
                ("172.16.0.0/12", "private"),
                ("192.168.0.0/16", "private"),
                ("100.64.0.0/10", "shared (carrier-grade NAT)"),
                ("169.254.0.0/16", "link-local"),
                ("192.0.0.0/24", "IETF protocol assignment"),
                ("192.0.2.0/24", "documentation (TEST-NET-1)"),
                ("198.51.100.0/24", "documentation (TEST-NET-2)"),
                ("203.0.113.0/24", "documentation (TEST-NET-3)"),
                ("198.18.0.0/15", "benchmarking"),
                ("224.0.0.0/4", "multicast"),
                ("255.255.255.255/32", "broadcast"),
                ("240.0.0.0/4", "reserved"),
            ];
            for (net, tag) in table {
                if in4(v, net) {
                    tags.push(tag);
                }
            }
        }
        IpAddr::V6(v) => {
            let table: &[(&str, &str)] = &[
                ("::/128", "unspecified"),
                ("::1/128", "loopback"),
                ("::ffff:0:0/96", "IPv4-mapped"),
                ("64:ff9b::/96", "NAT64"),
                ("100::/64", "discard-only"),
                ("2001::/32", "Teredo"),
                ("2001:db8::/32", "documentation"),
                ("3fff::/20", "documentation"),
                ("2002::/16", "6to4"),
                ("fc00::/7", "unique local (private)"),
                ("fe80::/10", "link-local"),
                ("ff00::/8", "multicast"),
            ];
            for (net, tag) in table {
                if in6(v, net) {
                    tags.push(tag);
                }
            }
            if tags.is_empty() && in6(v, "2000::/3") {
                tags.push("global unicast");
            }
        }
    }
    let scope = match tags.first() {
        None => "public",
        Some(t) if t.starts_with("private") || t.starts_with("unique local") || t.starts_with("shared") => "private",
        Some(&"global unicast") => "public",
        Some(t) => t,
    };
    if tags.is_empty() {
        tags.push("public");
    }
    (tags, scope)
}

fn bin_v4(ip: Ipv4Addr) -> String {
    ip.octets().iter().map(|o| format!("{o:08b}")).collect::<Vec<_>>().join(".")
}

fn expanded_v6(ip: Ipv6Addr) -> String {
    ip.segments().iter().map(|s| format!("{s:04x}")).collect::<Vec<_>>().join(":")
}

fn reverse_name(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0])
        }
        IpAddr::V6(v) => {
            let hex: String = v.octets().iter().map(|b| format!("{b:02x}")).collect();
            let mut nibbles: Vec<String> = hex.chars().rev().map(|c| c.to_string()).collect();
            nibbles.push("ip6.arpa".into());
            nibbles.join(".")
        }
    }
}

fn parse_ip(s: &str) -> Result<IpAddr> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    let s = s.split('%').next().unwrap_or(s); // drop IPv6 zone id
    s.parse::<IpAddr>().map_err(|_| invalid(format!("{s:?} is not an IPv4 or IPv6 address")))
}

pub fn ip_info(input: &str) -> Result<Value> {
    let ip = parse_ip(input)?;
    let (tags, scope) = classify(ip);
    Ok(match ip {
        IpAddr::V4(v) => {
            let n = u32::from(v);
            let class = match v.octets()[0] {
                0..=127 => "A",
                128..=191 => "B",
                192..=223 => "C",
                224..=239 => "D (multicast)",
                _ => "E (reserved)",
            };
            json!({
                "address": v.to_string(),
                "version": 4,
                "scope": scope,
                "tags": tags,
                "integer": n,
                "hex": format!("0x{n:08X}"),
                "binary": bin_v4(v),
                "historicalClass": class,
                "ipv4Mapped": v.to_ipv6_mapped().to_string(),
                "reverseDns": reverse_name(ip),
            })
        }
        IpAddr::V6(v) => {
            let embedded = v.to_ipv4_mapped().or_else(|| {
                let s = v.segments();
                if s[0] == 0x2002 {
                    Some(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8))
                } else if in6(v, "64:ff9b::/96") {
                    let o = v.octets();
                    Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
                } else {
                    None
                }
            });
            json!({
                "address": v.to_string(),
                "version": 6,
                "scope": scope,
                "tags": tags,
                "expanded": expanded_v6(v),
                "integer": u128::from(v).to_string(),
                "hex": format!("0x{:032x}", u128::from(v)),
                "embeddedIpv4": embedded.map(|e| e.to_string()),
                "reverseDns": reverse_name(ip),
            })
        }
    })
}

// ── CIDR ─────────────────────────────────────────────────────────────────────────────────────────

fn parse_net(input: &str) -> Result<(IpAddr, IpNet)> {
    let s = input.trim();
    if s.is_empty() {
        return Err(invalid("enter a network like 10.0.0.0/24, 192.168.1.10 255.255.255.0 or 2001:db8::/48"));
    }
    let (addr, mask) = match s.split_once('/').or_else(|| s.split_once(char::is_whitespace)) {
        Some((a, m)) => (a.trim(), Some(m.trim())),
        None => (s, None),
    };
    let ip = parse_ip(addr)?;
    let net = match (ip, mask) {
        (IpAddr::V4(v), Some(m)) if m.contains('.') => {
            let mask: Ipv4Addr = m.parse().map_err(|_| invalid(format!("bad netmask {m}")))?;
            IpNet::V4(Ipv4Net::with_netmask(v, mask).map_err(|_| invalid(format!("{m} is not a contiguous netmask")))?)
        }
        (_, Some(m)) => {
            let prefix: u8 = m.parse().map_err(|_| invalid(format!("bad prefix length {m}")))?;
            IpNet::new(ip, prefix).map_err(|_| invalid(format!("prefix /{prefix} is too long for {}", if ip.is_ipv4() { "IPv4 (max 32)" } else { "IPv6 (max 128)" })))?
        }
        (IpAddr::V4(v), None) => IpNet::V4(Ipv4Net::new(v, 32).map_err(|e| invalid(e.to_string()))?),
        (IpAddr::V6(v), None) => IpNet::V6(Ipv6Net::new(v, 128).map_err(|e| invalid(e.to_string()))?),
    };
    Ok((ip, net))
}

pub fn cidr(input: &str, contains: &str) -> Result<Value> {
    let (ip, net) = parse_net(input)?;
    let trunc = net.trunc();
    let (_, scope) = classify(trunc.network());
    let mut out = match trunc {
        IpNet::V4(n) => {
            let prefix = n.prefix_len();
            let total = 1u64 << (32 - prefix);
            let (first, last, usable) = match prefix {
                32 => (n.network(), n.network(), 1),
                31 => (n.network(), n.broadcast(), 2),
                _ => (Ipv4Addr::from(u32::from(n.network()) + 1), Ipv4Addr::from(u32::from(n.broadcast()) - 1), total - 2),
            };
            json!({
                "version": 4,
                "cidr": n.to_string(),
                "network": n.network().to_string(),
                "prefix": prefix,
                "netmask": n.netmask().to_string(),
                "wildcard": n.hostmask().to_string(),
                "broadcast": (prefix < 31).then(|| n.broadcast().to_string()),
                "firstHost": first.to_string(),
                "lastHost": last.to_string(),
                "totalAddresses": total,
                "usableHosts": usable,
                "binaryNetmask": bin_v4(n.netmask()),
                "scope": scope,
            })
        }
        IpNet::V6(n) => {
            let prefix = n.prefix_len();
            let total = if prefix == 0 { "340282366920938463463374607431768211456".to_string() } else { (1u128 << (128 - prefix)).to_string() };
            json!({
                "version": 6,
                "cidr": n.to_string(),
                "network": n.network().to_string(),
                "prefix": prefix,
                "netmask": n.netmask().to_string(),
                "firstHost": n.network().to_string(),
                "lastHost": n.broadcast().to_string(),
                "lastHostExpanded": expanded_v6(n.broadcast()),
                "totalAddresses": total,
                "subnets64": if prefix <= 64 { json!(format!("{}", 1u128 << (64 - prefix))) } else { json!(null) },
                "scope": scope,
            })
        }
    };
    out["input"] = json!(input);
    out["address"] = json!(ip.to_string());
    if net.addr() != trunc.network() {
        out["note"] = json!(format!("{} is a host inside {}", ip, trunc));
    }
    if !contains.is_empty() {
        let probe = parse_ip(contains)?;
        out["contains"] = json!({ "address": probe.to_string(), "inNetwork": trunc.contains(&probe) });
    }
    Ok(out)
}

// ── DNS ──────────────────────────────────────────────────────────────────────────────────────────

fn clean_host(input: &str) -> Result<String> {
    let mut s = input.trim();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest;
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    // host:port (but leave bare IPv6 alone)
    let s = if s.starts_with('[') {
        s.trim_start_matches('[').split(']').next().unwrap_or("")
    } else if s.matches(':').count() == 1 {
        s.split(':').next().unwrap_or("")
    } else {
        s
    };
    if s.is_empty() || s.contains(char::is_whitespace) {
        return Err(invalid("enter a host name like example.com"));
    }
    Ok(s.trim_end_matches('.').to_string())
}

pub fn dns_lookup(input: &str, kind: &str) -> Result<Value> {
    use hickory_resolver::proto::rr::RecordType;
    let host = clean_host(input)?;
    let reverse = host.parse::<IpAddr>().ok();
    let types: Vec<RecordType> = if let Some(_ip) = reverse {
        vec![RecordType::PTR]
    } else {
        match kind.to_ascii_uppercase().as_str() {
            "" | "ALL" => vec![RecordType::A, RecordType::AAAA, RecordType::CNAME, RecordType::MX, RecordType::TXT, RecordType::NS],
            "A" => vec![RecordType::A],
            "AAAA" => vec![RecordType::AAAA],
            "CNAME" => vec![RecordType::CNAME],
            "MX" => vec![RecordType::MX],
            "TXT" => vec![RecordType::TXT],
            "NS" => vec![RecordType::NS],
            "SOA" => vec![RecordType::SOA],
            "CAA" => vec![RecordType::CAA],
            "PTR" => vec![RecordType::PTR],
            other => return Err(invalid(format!("unsupported record type {other}"))),
        }
    };
    let name = match reverse {
        Some(ip) => format!("{}.", reverse_name(ip)),
        None => format!("{host}."),
    };
    // Own thread + runtime: safe to call from sync code and from inside another async runtime.
    let started = Instant::now();
    let results = std::thread::Builder::new()
        .name("devall-dns".into())
        .spawn(move || -> Result<Vec<Value>> {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| invalid(e.to_string()))?;
            rt.block_on(async move {
                let mut builder = hickory_resolver::TokioResolver::builder_tokio().map_err(|e| invalid(format!("cannot read the system DNS configuration: {e}")))?;
                builder.options_mut().timeout = Duration::from_secs(4);
                builder.options_mut().attempts = 2;
                let resolver = builder.build().map_err(|e| invalid(format!("cannot start the resolver: {e}")))?;
                let mut tasks = Vec::new();
                for t in types {
                    let r = resolver.clone();
                    let n = name.clone();
                    tasks.push((t, tokio::spawn(async move { r.lookup(n.as_str(), t).await })));
                }
                let mut out = Vec::new();
                for (t, task) in tasks {
                    let res = tokio::time::timeout(NET_TIMEOUT, task).await;
                    let entry = match res {
                        Ok(Ok(Ok(lookup))) => {
                            let records: Vec<Value> = lookup
                                .answers()
                                .iter()
                                .filter(|r| r.record_type() == t)
                                .map(|r| json!({ "name": r.name.to_string(), "ttl": r.ttl, "value": r.data.to_string() }))
                                .collect();
                            json!({ "type": t.to_string(), "records": records })
                        }
                        Ok(Ok(Err(e))) => {
                            let msg = e.to_string();
                            if msg.to_ascii_lowercase().contains("no record") || msg.contains("NXDomain") || msg.contains("no records") {
                                json!({ "type": t.to_string(), "records": [], "note": msg })
                            } else {
                                json!({ "type": t.to_string(), "records": [], "error": msg })
                            }
                        }
                        Ok(Err(e)) => json!({ "type": t.to_string(), "records": [], "error": e.to_string() }),
                        Err(_) => json!({ "type": t.to_string(), "records": [], "error": "timed out" }),
                    };
                    out.push(entry);
                }
                Ok(out)
            })
        })
        .map_err(|e| invalid(e.to_string()))?
        .join()
        .map_err(|_| invalid("DNS lookup thread panicked"))??;
    Ok(json!({
        "query": host,
        "reverse": reverse.is_some(),
        "elapsedMs": started.elapsed().as_millis() as u64,
        "resolver": "system configuration",
        "results": results,
    }))
}

// ── TLS chain ────────────────────────────────────────────────────────────────────────────────────

/// Delegates to the OS verifier but records its verdict instead of aborting the handshake, so an
/// invalid chain can still be inspected. No application data is ever sent on the connection.
#[derive(Debug)]
struct Recorder {
    inner: Arc<dyn rustls::client::danger::ServerCertVerifier>,
    verdict: Mutex<Option<std::result::Result<(), String>>>,
}

impl rustls::client::danger::ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let r = self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now);
        if let Ok(mut v) = self.verdict.lock() {
            *v = Some(r.map(|_| ()).map_err(|e| e.to_string()));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

fn split_host_port(input: &str) -> Result<(String, u16)> {
    let mut s = input.trim();
    let mut default_port = 443;
    if let Some((scheme, rest)) = s.split_once("://") {
        default_port = match scheme.to_ascii_lowercase().as_str() {
            "ldaps" => 636,
            "smtps" => 465,
            "imaps" => 993,
            "pop3s" => 995,
            _ => 443,
        };
        s = rest;
    }
    let s = s.split(['/', '?', '#']).next().unwrap_or("");
    let s = s.rsplit_once('@').map_or(s, |(_, h)| h);
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or_else(|| invalid("unclosed [ in IPv6 address"))?;
        (h.to_string(), after.strip_prefix(':').map(str::to_string))
    } else if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap_or((s, ""));
        (h.to_string(), Some(p.to_string()))
    } else {
        (s.to_string(), None)
    };
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(invalid("enter host[:port], e.g. example.com:443"));
    }
    let port = match port {
        Some(p) if !p.is_empty() => p.parse().map_err(|_| invalid(format!("bad port {p}")))?,
        _ => default_port,
    };
    Ok((host, port))
}

pub fn tls_chain(input: &str) -> Result<Value> {
    let (host, port) = split_host_port(input)?;
    tls_chain_with(&host, port, None)
}

/// `extra_roots` lets tests trust a local CA; production uses the OS trust store only.
pub(crate) fn tls_chain_with(host: &str, port: u16, extra_roots: Option<Vec<rustls::pki_types::CertificateDer<'static>>>) -> Result<Value> {
    use std::io::Write;

    let started = Instant::now();
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| invalid(format!("cannot resolve {host}: {e}")))?
        .next()
        .ok_or_else(|| invalid(format!("{host} has no addresses")))?;
    let mut sock = TcpStream::connect_timeout(&addr, NET_TIMEOUT).map_err(|e| invalid(format!("cannot connect to {addr}: {e}")))?;
    sock.set_read_timeout(Some(NET_TIMEOUT)).ok();
    sock.set_write_timeout(Some(NET_TIMEOUT)).ok();

    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let inner: Arc<dyn rustls::client::danger::ServerCertVerifier> = match extra_roots {
        None => Arc::new(rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|e| invalid(format!("cannot load the OS trust store: {e}")))?),
        Some(roots) => {
            let mut store = rustls::RootCertStore::empty();
            for r in roots {
                store.add(r).map_err(|e| invalid(e.to_string()))?;
            }
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(store), provider.clone()).build().map_err(|e| invalid(e.to_string()))?
        }
    };
    let recorder = Arc::new(Recorder { inner, verdict: Mutex::new(None) });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| invalid(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| invalid(format!("bad server name {host}: {e}")))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(config), server_name).map_err(|e| invalid(e.to_string()))?;
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| invalid(format!("TLS handshake with {addr} failed: {e}")))?;
    }
    let handshake_ms = started.elapsed().as_millis() as u64;
    let protocol = conn.protocol_version().map(|v| format!("{v:?}").replace('_', "."));
    let suite = conn.negotiated_cipher_suite().map(|s| format!("{:?}", s.suite()));
    let alpn = conn.alpn_protocol().map(|p| String::from_utf8_lossy(p).into_owned());
    let ders: Vec<Vec<u8>> = conn.peer_certificates().unwrap_or_default().iter().map(|c| c.as_ref().to_vec()).collect();
    conn.send_close_notify();
    let _ = conn.complete_io(&mut sock);
    let _ = sock.flush();

    let now = chrono::Utc::now().timestamp();
    let certs: Vec<Value> = ders.iter().map(|d| x509::summarize(d, now)).collect::<Result<_>>()?;
    let verdict = recorder.verdict.lock().ok().and_then(|v| v.clone());
    let (verified, error) = match verdict {
        Some(Ok(())) => (true, None),
        Some(Err(e)) => (false, Some(e)),
        None => (false, Some("the server presented no certificate".to_string())),
    };
    Ok(json!({
        "host": host,
        "port": port,
        "address": addr.to_string(),
        "protocol": protocol,
        "cipherSuite": suite,
        "alpn": alpn,
        "handshakeMs": handshake_ms,
        "verified": verified,
        "verificationError": error,
        "trustStore": "operating system",
        "certificates": certs,
        "chainWarnings": x509::chain_notes(&certs),
    }))
}
