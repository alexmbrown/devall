import type { ServeEvent, StubMapping } from './types';
import { requestToStub, type ToStubOptions, DEFAULT_TO_STUB } from './request-to-stub';

interface HarHeader {
  name: string;
  value: string;
}
interface HarEntry {
  startedDateTime: string;
  time: number;
  request: { method: string; url: string; httpVersion?: string; headers: HarHeader[]; queryString?: HarHeader[]; postData?: { mimeType?: string; text?: string } };
  response: { status: number; statusText?: string; headers: HarHeader[]; content: { size?: number; mimeType?: string; text?: string; encoding?: string } };
  timings?: Record<string, number>;
}
export interface Har {
  log: { version: string; creator: { name: string; version: string }; entries: HarEntry[] };
}

const toHeaders = (h?: Record<string, string | string[]>): HarHeader[] =>
  Object.entries(h ?? {}).flatMap(([name, v]) => (Array.isArray(v) ? v : [v]).map((value) => ({ name, value })));

export function journalToHar(events: ServeEvent[]): Har {
  return {
    log: {
      version: '1.2',
      creator: { name: 'WireMock UI', version: '1' },
      entries: events.map((e) => ({
        startedDateTime: new Date(e.request.loggedDate ?? Date.now()).toISOString(),
        time: e.timing?.totalTime ?? 0,
        request: {
          method: e.request.method,
          url: e.request.absoluteUrl ?? e.request.url,
          httpVersion: e.request.protocol ?? 'HTTP/1.1',
          headers: toHeaders(e.request.headers),
          queryString: [...new URLSearchParams(e.request.url.split('?')[1] ?? '')].map(([name, value]) => ({ name, value })),
          postData: e.request.body ? { mimeType: String(e.request.headers?.['Content-Type'] ?? ''), text: e.request.body } : undefined,
        },
        response: {
          status: e.response?.status ?? 0,
          headers: toHeaders(e.response?.headers),
          content: e.response?.bodyAsBase64 && !e.response.body ? { text: e.response.bodyAsBase64, encoding: 'base64' } : { text: e.response?.body ?? '' },
        },
        timings: { send: 0, wait: e.timing?.serveTime ?? 0, receive: e.timing?.responseSendTime ?? 0 },
      })),
    },
  };
}

/**
 * Convert a HAR (browser DevTools → "Save all as HAR") into stubs — record
 * real traffic without running a proxy.
 */
export function harToStubs(har: Har, opts: Partial<ToStubOptions> & { filter?: string; dedupe?: boolean; stripOrigin?: boolean } = {}): StubMapping[] {
  const out: StubMapping[] = [];
  const seen = new Set<string>();
  for (const e of har.log?.entries ?? []) {
    let u: URL;
    try {
      u = new URL(e.request.url);
    } catch {
      continue;
    }
    if (opts.filter && !e.request.url.includes(opts.filter)) continue;
    const mime = e.response.content?.mimeType ?? '';
    if (/^(image|font)\//.test(mime) || /\.(js|css|woff2?|png|svg|ico)(\?|$)/.test(u.pathname)) continue;
    const key = `${e.request.method} ${u.pathname}${u.search}`;
    if (opts.dedupe !== false && seen.has(key)) continue;
    seen.add(key);
    const headers: Record<string, string> = {};
    for (const h of e.request.headers ?? []) headers[h.name] = h.value;
    const rh: Record<string, string> = {};
    for (const h of e.response.headers ?? []) if (!h.name.startsWith(':')) rh[h.name] = h.value;
    const content = e.response.content ?? {};
    const stub = requestToStub(
      { method: e.request.method, url: u.pathname + u.search, headers, body: e.request.postData?.text },
      {
        status: e.response.status,
        headers: rh,
        body: content.encoding === 'base64' ? undefined : content.text,
        bodyAsBase64: content.encoding === 'base64' ? content.text : undefined,
      },
      { ...DEFAULT_TO_STUB, ...opts },
    );
    stub.metadata = { 'wiremock-gui': { folder: `/har/${u.host}` } };
    out.push(stub);
  }
  return out;
}
