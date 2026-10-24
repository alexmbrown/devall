/**
 * Recognizes values worth decoding inline (JWTs, timestamps, Base64, gzip, URLs, JSON, certificates,
 * IPs / CIDR blocks, cron expressions) and picks the
 * utility op that decodes them. Shared by clipboard detection on the Utilities page and by the
 * "Detected values" panel on responses.
 */

export interface Detection {
  kind: 'jwt' | 'timestamp' | 'base64' | 'url' | 'json' | 'uuid' | 'cert' | 'cidr' | 'ip' | 'cron' | 'gzip';
  label: string;
  op: string;
  value: string;
}

const JWT_RE = /\beyJ[A-Za-z0-9_-]{5,}\.eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]*/g;
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const B64_RE = /^[A-Za-z0-9+/_-]+={0,2}$/;
const IPV4_RE = /^(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;
const PEM_CERT_RE = /^-----BEGIN (?:TRUSTED )?CERTIFICATE-----/;
const GZIP_B64_RE = /^H4sI[A-Za-z0-9+/_-]{8,}={0,2}$/;
const CRON_NICKNAME_RE = /^@(?:yearly|annually|monthly|weekly|daily|midnight|hourly)$/i;

/** Loose IPv6 check: hex groups with `::` or exactly eight groups (so `12:30:45` is not an IP). */
export function isIpv6(s: string): boolean {
  const addr = s.replace(/%[\w.-]+$/, '');
  if (!/^[0-9a-f:.]+$/i.test(addr) || /:::/.test(addr)) return false;
  const colons = (addr.match(/:/g) ?? []).length;
  if (!(addr.includes('::') ? colons >= 2 && colons <= 7 : colons === 7)) return false;
  return addr.split(':').every((g) => g === '' || /^[0-9a-f]{1,4}$/i.test(g) || IPV4_RE.test(g));
}

export function isIp(s: string): boolean {
  return IPV4_RE.test(s) || isIpv6(s);
}

/** IPv4/IPv6 network in CIDR notation. */
export function isCidr(s: string): boolean {
  const m = /^(.+)\/(\d{1,3})$/.exec(s);
  if (!m) return false;
  const prefix = Number(m[2]);
  return IPV4_RE.test(m[1]) ? prefix <= 32 : isIpv6(m[1]) && prefix <= 128;
}

/** 5–7 cron fields (or `@daily`-style), with at least one `*`, `/` or `?` so plain numbers don't count. */
export function looksLikeCron(s: string): boolean {
  if (CRON_NICKNAME_RE.test(s)) return true;
  const fields = s.split(/\s+/);
  if (fields.length < 5 || fields.length > 7) return false;
  const ok = fields.every((f) => /^[\d*?/,#LW-]+$/.test(f) || /^[A-Za-z]{3}(?:[-,#][A-Za-z0-9]{1,3})*$/.test(f));
  return ok && fields.some((f) => /[*/?]/.test(f));
}

/** Plausible epoch: 2001-09-09 … 2286-11-20 in seconds, or the same range in milliseconds. */
export function isEpoch(s: string): boolean {
  if (!/^\d{10}$|^\d{13}$/.test(s)) return false;
  const n = Number(s.length === 13 ? s.slice(0, 10) : s);
  return n >= 1_000_000_000 && n <= 9_999_999_999;
}

function looksBase64(s: string): boolean {
  if (s.length < 12 || s.length % 4 === 1 || !B64_RE.test(s)) return false;
  // Require a mix of character classes so plain words/hex aren't flagged.
  const classes = [/[a-z]/, /[A-Z]/, /\d/].filter((r) => r.test(s)).length;
  if (classes < 2 || /^[0-9a-f]+$/i.test(s)) return false;
  try {
    const text = atob(s.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(s.length / 4) * 4, '='));
    // Mostly printable once decoded.
    const printable = [...text].filter((c) => c >= ' ' && c <= '~').length;
    return printable / Math.max(1, text.length) > 0.85;
  } catch {
    return false;
  }
}

/** Best single interpretation of a pasted value. */
export function detect(raw: string): Detection | null {
  const s = raw.trim();
  if (!s) return null;
  const bearer = s.replace(/^Bearer\s+/i, '');
  if (/^eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*$/.test(bearer)) return { kind: 'jwt', label: 'JWT', op: 'jwt.decode', value: bearer };
  if (PEM_CERT_RE.test(s)) return { kind: 'cert', label: 'X.509 certificate', op: 'x509.decode', value: s };
  if (isEpoch(s)) return { kind: 'timestamp', label: 'Unix timestamp', op: 'time.convert', value: s };
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(s)) return { kind: 'timestamp', label: 'ISO 8601 date', op: 'time.convert', value: s };
  if (UUID_RE.test(s)) return { kind: 'uuid', label: 'UUID', op: 'text.stats', value: s };
  if (/^https?:\/\/\S+$/i.test(s)) return { kind: 'url', label: 'URL', op: 'url.parse', value: s };
  if (/^[[{]/.test(s)) {
    try {
      JSON.parse(s);
      return { kind: 'json', label: 'JSON', op: 'json.format', value: s };
    } catch {
      /* not JSON */
    }
  }
  if (isCidr(s)) return { kind: 'cidr', label: 'CIDR block', op: 'cidr.calc', value: s };
  if (isIp(s)) return { kind: 'ip', label: 'IP address', op: 'ip.info', value: s };
  if (looksLikeCron(s)) return { kind: 'cron', label: 'Cron expression', op: 'cron.explain', value: s };
  if (GZIP_B64_RE.test(s.replace(/\s+/g, ''))) return { kind: 'gzip', label: 'Gzip (Base64)', op: 'gzip.decompress', value: s };
  if (looksBase64(s)) return { kind: 'base64', label: 'Base64', op: 'base64.decode', value: s };
  if (/%[0-9A-Fa-f]{2}/.test(s)) return { kind: 'url', label: 'URL-encoded text', op: 'url.decode', value: s };
  return null;
}

/** Every JWT and epoch timestamp inside a larger text (headers, bodies), deduplicated. */
export function findValues(text: string, limit = 20): Detection[] {
  const out: Detection[] = [];
  const seen = new Set<string>();
  const add = (d: Detection) => {
    if (!seen.has(d.value) && out.length < limit) {
      seen.add(d.value);
      out.push(d);
    }
  };
  for (const m of text.matchAll(JWT_RE)) add({ kind: 'jwt', label: 'JWT', op: 'jwt.decode', value: m[0] });
  for (const m of text.matchAll(/"(?:exp|iat|nbf|expires_at|expiresAt|created_at|createdAt|updated_at|updatedAt|timestamp)"\s*:\s*(\d{10}|\d{13})\b/g))
    if (isEpoch(m[1])) add({ kind: 'timestamp', label: 'Timestamp', op: 'time.convert', value: m[1] });
  return out;
}
