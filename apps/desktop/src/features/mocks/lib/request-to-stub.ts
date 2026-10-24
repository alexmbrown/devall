import type { ContentPattern, LoggedRequest, LoggedResponse, StubMapping } from './types';

export interface ToStubOptions {
  matchQuery: boolean;
  /** Request header names (lower-case) to match exactly. */
  headers: string[];
  body: 'none' | 'equalToJson' | 'equalTo' | 'contains-json-keys';
  ignoreExtraElements: boolean;
  /** Use the real response when available (serve event / recording). */
  useResponse: boolean;
  urlMode: 'urlPath' | 'url' | 'urlPathPattern';
}

export const DEFAULT_TO_STUB: ToStubOptions = {
  matchQuery: true,
  headers: [],
  body: 'equalToJson',
  ignoreExtraElements: true,
  useResponse: true,
  urlMode: 'urlPath',
};

const SKIP_RESPONSE_HEADERS = /^(content-length|transfer-encoding|date|connection|server|matched-stub-id|vary|content-encoding)$/i;

/** Turn a journal entry into a stub that would match it again. */
export function requestToStub(req: LoggedRequest, response: LoggedResponse | undefined, o: ToStubOptions = DEFAULT_TO_STUB): StubMapping {
  const [path, qs] = req.url.split('?');
  const request: StubMapping['request'] = { method: req.method };
  if (o.urlMode === 'url') request.url = req.url;
  else if (o.urlMode === 'urlPathPattern') request.urlPathPattern = path.replace(/\/\d+(?=\/|$)/g, '/[0-9]+').replace(/\/[0-9a-f]{8}-[0-9a-f-]{27}(?=\/|$)/gi, '/[0-9a-f-]{36}');
  else request.urlPath = path;

  if (o.urlMode !== 'url' && o.matchQuery && qs) {
    const q: Record<string, ContentPattern> = {};
    const params = new URLSearchParams(qs);
    for (const k of new Set(params.keys())) {
      const vals = params.getAll(k);
      q[k] = vals.length > 1 ? { hasExactly: vals.map((v) => ({ equalTo: v })) } : { equalTo: vals[0] };
    }
    if (Object.keys(q).length) request.queryParameters = q;
  }
  if (o.headers.length) {
    const h: Record<string, ContentPattern> = {};
    for (const [k, v] of Object.entries(req.headers ?? {})) if (o.headers.includes(k.toLowerCase())) h[k] = { equalTo: Array.isArray(v) ? v[0] : v };
    if (Object.keys(h).length) request.headers = h;
  }
  const body = req.body;
  if (body && o.body !== 'none') {
    let json: unknown;
    try {
      json = JSON.parse(body);
    } catch {
      json = undefined;
    }
    if ((o.body === 'equalToJson' || o.body === 'contains-json-keys') && json !== undefined) {
      request.bodyPatterns = [{ equalToJson: json, ...(o.ignoreExtraElements || o.body === 'contains-json-keys' ? { ignoreExtraElements: true } : {}) }];
    } else request.bodyPatterns = [{ equalTo: body }];
  }

  const r: StubMapping['response'] = {};
  if (o.useResponse && response && !response.fault) {
    r.status = response.status;
    const headers: Record<string, string> = {};
    for (const [k, v] of Object.entries(response.headers ?? {})) if (!SKIP_RESPONSE_HEADERS.test(k)) headers[k] = Array.isArray(v) ? v[0] : v;
    if (Object.keys(headers).length) r.headers = headers;
    const text = response.body ?? (response.bodyAsBase64 ? undefined : '');
    const ct = Object.entries(headers).find(([k]) => k.toLowerCase() === 'content-type')?.[1] ?? '';
    if (text) {
      if (ct.includes('json')) {
        try {
          r.jsonBody = JSON.parse(text);
        } catch {
          r.body = text;
        }
      } else r.body = text;
    } else if (response.bodyAsBase64 && !/^text\//.test(ct)) r.base64Body = response.bodyAsBase64;
  } else {
    r.status = 200;
    r.headers = { 'Content-Type': 'application/json' };
    r.jsonBody = {};
  }
  return { name: `${req.method} ${path}`, request, response: r };
}
