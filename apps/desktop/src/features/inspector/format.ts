import type { BodyView, BreakpointEdit, HeaderList, MessageView, PausedExchange } from '@/lib/api/proxy';
import { formatBytes } from '@/components/ui/misc';

export function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return '—';
  if (ms < 1) return `${ms.toFixed(2)} ms`;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
}

export function fmtClock(ts: number): string {
  const d = new Date(ts);
  return `${d.toLocaleTimeString([], { hour12: false })}.${String(d.getMilliseconds()).padStart(3, '0')}`;
}

export function isJsonType(ct?: string | null): boolean {
  return !!ct && /json/i.test(ct);
}

/** Pretty-prints JSON bodies; anything else is returned unchanged. */
export function prettyBody(text: string, contentType?: string | null): string {
  const t = text.trimStart();
  if (!isJsonType(contentType) && !(t.startsWith('{') || t.startsWith('['))) return text;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

export function binaryPlaceholder(b: BodyView): string {
  return `«binary body, ${formatBytes(b.captured)} — kept as is»`;
}

/** The editable/displayable body text (pretty JSON, placeholder for binary). */
export function bodyText(b: BodyView): string {
  if (b.text != null) return prettyBody(b.text, b.contentType);
  if (b.base64 != null) return binaryPlaceholder(b);
  return '';
}

function headerLines(h: HeaderList): string {
  return h.map(([k, v]) => `${k}: ${v}`).join('\n');
}

/** A message as HTTP/1.1-style text (for "Raw" views and the breakpoint editor). */
export function rawMessage(m: MessageView, kind: 'request' | 'response', pathOnly = false): string {
  let first: string;
  if (kind === 'request') {
    let target = m.url ?? '/';
    if (pathOnly) {
      try {
        const u = new URL(target);
        target = u.pathname + u.search;
      } catch {
        /* keep */
      }
    }
    first = `${m.method ?? 'GET'} ${target} ${m.httpVersion}`;
  } else {
    first = `${m.httpVersion} ${m.status ?? ''} ${m.reason ?? ''}`.trimEnd();
  }
  const head = [first, headerLines(m.headers)].filter(Boolean).join('\n');
  const body = m.body.text != null ? m.body.text : m.body.base64 != null ? binaryPlaceholder(m.body) : '';
  return body ? `${head}\n\n${body}` : `${head}\n`;
}

export function pausedText(p: PausedExchange): string {
  const first = p.phase === 'request' ? `${p.method} ${p.url} HTTP/1.1` : `HTTP/1.1 ${p.status ?? 200}`;
  const head = [first, headerLines(p.headers)].filter(Boolean).join('\n');
  return `${head}\n\n${bodyText(p.body)}`;
}

interface Parsed {
  method?: string;
  url?: string;
  status?: number;
  headers: HeaderList;
  body: string;
}

export function parseMessage(text: string, phase: 'request' | 'response'): Parsed | string {
  const norm = text.replace(/\r\n/g, '\n');
  const split = norm.indexOf('\n\n');
  const head = split >= 0 ? norm.slice(0, split) : norm.replace(/\n+$/, '');
  const body = split >= 0 ? norm.slice(split + 2) : '';
  const lines = head.split('\n');
  const first = (lines.shift() ?? '').trim();
  const out: Parsed = { headers: [], body };
  if (phase === 'request') {
    const m = /^([A-Za-z]+)\s+(\S+)(?:\s+HTTP\/[\d.]+)?$/.exec(first);
    if (!m) return 'The first line must be "METHOD URL", e.g. "GET https://example.com/ HTTP/1.1".';
    out.method = m[1].toUpperCase();
    out.url = m[2];
  } else {
    const m = /^(?:HTTP\/[\d.]+\s+)?(\d{3})\b/.exec(first);
    if (!m) return 'The first line must be "HTTP/1.1 <status>", e.g. "HTTP/1.1 200 OK".';
    out.status = Number(m[1]);
  }
  for (const [i, line] of lines.entries()) {
    if (!line.trim()) continue;
    const c = line.indexOf(':');
    if (c <= 0) return `Header line ${i + 2} must be "Name: value".`;
    out.headers.push([line.slice(0, c).trim(), line.slice(c + 1).trim()]);
  }
  return out;
}

/** The minimal edit turning the paused message into `text` (undefined when nothing changed). */
export function editFor(p: PausedExchange, text: string): BreakpointEdit | undefined | string {
  const parsed = parseMessage(text, p.phase);
  if (typeof parsed === 'string') return parsed;
  const edit: BreakpointEdit = {};
  if (p.phase === 'request') {
    if (parsed.method && parsed.method !== p.method.toUpperCase()) edit.method = parsed.method;
    if (parsed.url && parsed.url !== p.url) edit.url = parsed.url;
  } else if (parsed.status && parsed.status !== p.status) edit.status = parsed.status;
  if (headerLines(parsed.headers) !== headerLines(p.headers)) edit.headers = parsed.headers;
  const original = bodyText(p.body);
  if (parsed.body !== original && !(p.body.base64 != null && parsed.body.trim() === binaryPlaceholder(p.body))) edit.body = parsed.body;
  return Object.keys(edit).length ? edit : undefined;
}

export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Classic 16-bytes-per-line hex dump (first `limit` bytes). */
export function hexDump(b64: string, limit = 64 * 1024): string {
  const bytes = base64ToBytes(b64).slice(0, limit);
  const lines: string[] = [];
  for (let off = 0; off < bytes.length; off += 16) {
    const chunk = bytes.slice(off, off + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, '0')).join(' ');
    const ascii = Array.from(chunk, (b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : '.')).join('');
    lines.push(`${off.toString(16).padStart(8, '0')}  ${hex.padEnd(47)}  ${ascii}`);
  }
  return lines.join('\n');
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
