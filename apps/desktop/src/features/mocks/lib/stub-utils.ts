import type { ContentPattern, MethodPattern, RequestPattern, StubMapping } from './types';

/**
 * Metadata conventions. All organisational data lives inside each stub's
 * `metadata` on the WireMock server so nothing needs syncing locally.
 *  - folder:  metadata["wiremock-gui"].folder  (compatible with holomekc/wiremock-gui)
 *             falls back to metadata.section (testsmith) when reading
 *  - ours:    metadata.wmui = { description, tags, disabled, proxyOff, ... }
 */
export const GUI_NS = 'wiremock-gui';
export const NS = 'wmui';
export const DISABLED_HEADER = 'X-WireMock-UI-Disabled';

export interface WmuiMeta {
  description?: string;
  tags?: string[];
  disabled?: boolean;
  proxyOff?: Pick<StubMapping['response'], 'proxyBaseUrl' | 'proxyUrlPrefixToRemove' | 'additionalProxyRequestHeaders' | 'removeProxyRequestHeaders'>;
  updatedAt?: string;
  createdAt?: string;
  [k: string]: unknown;
}

export const URL_KEYS = ['url', 'urlPath', 'urlPattern', 'urlPathPattern', 'urlPathTemplate'] as const;
export type UrlKey = (typeof URL_KEYS)[number];

export const URL_KEY_LABEL: Record<UrlKey, string> = {
  url: 'URL equals',
  urlPath: 'Path equals',
  urlPattern: 'URL regex',
  urlPathPattern: 'Path regex',
  urlPathTemplate: 'Path template',
};

export function urlKeyOf(r: RequestPattern): UrlKey | undefined {
  return URL_KEYS.find((k) => r[k] !== undefined);
}

export function urlOf(r: RequestPattern): string {
  const k = urlKeyOf(r);
  return k ? String(r[k]) : '';
}

export function stubId(s: StubMapping): string {
  return (s.id ?? s.uuid) as string;
}

/**
 * WireMock 4 dropped the `uuid` alias: it is not returned, and a stub sent with
 * only `uuid` gets a fresh random id. 3.x accepts `id` alone, so always send `id`.
 */
export function normalizeIdentity<T extends { id?: string; uuid?: string }>(s: T): T {
  if (!('uuid' in s)) return s;
  const { uuid, ...rest } = s;
  return (rest.id || !uuid ? rest : { ...rest, id: uuid }) as T;
}

// ---------------------------------------------------------------- methods

export const METHODS = ['ANY', 'GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS', 'TRACE'];
/** QUERY (RFC draft) is a built-in method from WireMock 4. */
export const METHODS_V4 = [...METHODS, 'QUERY'];

export function methodSetOf(m: MethodPattern | undefined): { kind: 'oneOf' | 'noneOf'; methods: string[] } | undefined {
  if (!m || typeof m !== 'object') return undefined;
  if ('oneOf' in m && Array.isArray(m.oneOf)) return { kind: 'oneOf', methods: m.oneOf.map(String) };
  if ('noneOf' in m && Array.isArray(m.noneOf)) return { kind: 'noneOf', methods: m.noneOf.map(String) };
  return undefined;
}

/** Short label: `GET`, `PUT|PATCH` (one of), `!GET|DELETE` (none of). */
export function methodLabel(m: MethodPattern | undefined): string {
  const set = methodSetOf(m);
  if (set) return `${set.kind === 'noneOf' ? '!' : ''}${set.methods.join('|')}`;
  return typeof m === 'string' && m ? m.toUpperCase() : 'ANY';
}

/** True when a request with `actual` method satisfies the method pattern. */
export function methodAccepts(m: MethodPattern | undefined, actual: string): boolean {
  const a = actual.toUpperCase();
  const set = methodSetOf(m);
  if (set) {
    const has = set.methods.some((x) => x.toUpperCase() === a);
    return set.kind === 'oneOf' ? has : !has;
  }
  const exp = typeof m === 'string' && m ? m.toUpperCase() : 'ANY';
  return exp === 'ANY' || exp === a;
}

// ---------------------------------------------------------------- metadata

/** Anything carrying WireMock `metadata` (HTTP stubs and WireMock 4 message stubs). */
type WithMetadata = { metadata?: Record<string, unknown> };

export function wmui(s: WithMetadata): WmuiMeta {
  return ((s.metadata?.[NS] as WmuiMeta) ?? {}) as WmuiMeta;
}

export function withWmui<T extends WithMetadata>(s: T, patch: Partial<WmuiMeta>): T {
  const cur = { ...wmui(s), ...patch };
  for (const k of Object.keys(cur)) if (cur[k] === undefined) delete cur[k];
  const metadata = { ...(s.metadata ?? {}) };
  if (Object.keys(cur).length) metadata[NS] = cur;
  else delete metadata[NS];
  return { ...s, metadata: Object.keys(metadata).length ? metadata : undefined };
}

export function folderOf(s: WithMetadata): string {
  const gui = s.metadata?.[GUI_NS] as { folder?: string } | undefined;
  const f = gui?.folder ?? (typeof s.metadata?.section === 'string' ? (s.metadata.section as string) : '');
  return normalizeFolder(f);
}

export function normalizeFolder(f?: string): string {
  return (f ?? '')
    .split('/')
    .map((x) => x.trim())
    .filter(Boolean)
    .join('/');
}

export function withFolder<T extends WithMetadata>(s: T, folder: string): T {
  const f = normalizeFolder(folder);
  const metadata = { ...(s.metadata ?? {}) };
  const gui = { ...((metadata[GUI_NS] as Record<string, unknown>) ?? {}) };
  if (f) gui.folder = `/${f}`;
  else delete gui.folder;
  if (Object.keys(gui).length) metadata[GUI_NS] = gui;
  else delete metadata[GUI_NS];
  if (typeof metadata.section === 'string') delete metadata.section;
  return { ...s, metadata: Object.keys(metadata).length ? metadata : undefined };
}

export function tagsOf(s: WithMetadata): string[] {
  const t = wmui(s).tags;
  if (Array.isArray(t)) return t.map(String);
  const legacy = s.metadata?.tags;
  return Array.isArray(legacy) ? legacy.map(String) : [];
}

export function isDisabled(s: StubMapping): boolean {
  return wmui(s).disabled === true;
}

/**
 * WireMock has no "disabled" flag, so we add a header matcher no real client
 * sends and mark the stub in metadata. Re-enabling removes both.
 */
export function setDisabled(s: StubMapping, disabled: boolean): StubMapping {
  const headers = { ...(s.request.headers ?? {}) };
  if (disabled) headers[DISABLED_HEADER] = { equalTo: 'true' };
  else delete headers[DISABLED_HEADER];
  const request = { ...s.request, headers: Object.keys(headers).length ? headers : undefined };
  if (!request.headers) delete request.headers;
  return withWmui({ ...s, request }, { disabled: disabled || undefined });
}

export function isProxy(s: StubMapping): boolean {
  return !!s.response?.proxyBaseUrl;
}

export function hasParkedProxy(s: StubMapping): boolean {
  return !!wmui(s).proxyOff?.proxyBaseUrl;
}

/** Toggle between proxying and serving the stub's own response; proxy config is parked in metadata. */
export function toggleProxy(s: StubMapping): StubMapping {
  const r = { ...s.response };
  if (r.proxyBaseUrl) {
    const proxyOff = {
      proxyBaseUrl: r.proxyBaseUrl,
      proxyUrlPrefixToRemove: r.proxyUrlPrefixToRemove,
      additionalProxyRequestHeaders: r.additionalProxyRequestHeaders,
      removeProxyRequestHeaders: r.removeProxyRequestHeaders,
    };
    delete r.proxyBaseUrl;
    delete r.proxyUrlPrefixToRemove;
    delete r.additionalProxyRequestHeaders;
    delete r.removeProxyRequestHeaders;
    if (r.status === undefined) r.status = 200;
    return withWmui({ ...s, response: r }, { proxyOff });
  }
  const parked = wmui(s).proxyOff;
  if (!parked) return s;
  return withWmui({ ...s, response: { ...r, ...stripUndefined(parked) } }, { proxyOff: undefined });
}

export function stripUndefined<T extends Record<string, unknown>>(o: T): T {
  const out = { ...o };
  for (const k of Object.keys(out)) if (out[k] === undefined) delete out[k];
  return out;
}

export function displayName(s: StubMapping): string {
  return s.name || `${methodLabel(s.request?.method)} ${urlOf(s.request ?? {}) || '(any URL)'}`;
}

export interface StubTraits {
  proxy: boolean;
  fault?: string;
  delay: boolean;
  templated: boolean;
  file: boolean;
  webhook: boolean;
  scenario: boolean;
  persistent: boolean;
  disabled: boolean;
  bodyMatch: boolean;
  /** WireMock 4: the stub opens a WebSocket or SSE channel instead of answering. */
  channel?: 'websocket' | 'sse';
}

export function traitsOf(s: StubMapping): StubTraits {
  const r = s.response ?? {};
  const listeners = [...(Array.isArray(s.serveEventListeners) ? s.serveEventListeners : []), ...(Array.isArray(s.postServeActions) ? s.postServeActions : [])];
  return {
    proxy: !!r.proxyBaseUrl,
    fault: r.fault,
    delay: !!(r.fixedDelayMilliseconds || r.delayDistribution || r.chunkedDribbleDelay),
    templated: !!r.transformers?.includes('response-template'),
    file: !!r.bodyFileName,
    webhook: listeners.some((l) => l?.name === 'webhook') || (!!s.postServeActions && !Array.isArray(s.postServeActions) && 'webhook' in s.postServeActions),
    scenario: !!s.scenarioName,
    persistent: !!s.persistent,
    disabled: isDisabled(s),
    bodyMatch: !!s.request?.bodyPatterns?.length,
    channel: r.openWebsocketChannel ? 'websocket' : r.openSseChannel ? 'sse' : undefined,
  };
}

/** Keys WireMock adds on read that should not be sent back; `uuid` becomes `id` (see normalizeIdentity). */
export function cleanForSave(s: StubMapping): StubMapping {
  const out = normalizeIdentity(structuredClone(s));
  delete out.insertionIndex;
  if (out.response) delete out.response.fromConfiguredStub;
  if (out.metadata && !Object.keys(out.metadata).length) delete out.metadata;
  return out;
}

/** Remove server-assigned identity so the stub can be created elsewhere / duplicated. */
export function stripIdentity(s: StubMapping): StubMapping {
  const out = cleanForSave(s);
  delete out.id;
  delete out.uuid;
  return out;
}

export const MATCHER_OPS = [
  'equalTo',
  'contains',
  'doesNotContain',
  'matches',
  'doesNotMatch',
  'absent',
  'equalToJson',
  'matchesJsonPath',
  'matchesJsonSchema',
  'equalToXml',
  'matchesXPath',
  'before',
  'after',
  'equalToDateTime',
  'binaryEqualTo',
  'equalToNumber',
  'greaterThanNumber',
  'greaterThanEqualNumber',
  'lessThanNumber',
  'lessThanEqualNumber',
] as const;
export type MatcherOp = (typeof MATCHER_OPS)[number];

/** WireMock 4 only; 3.x rejects them with 422. */
export const NUMBER_OPS = ['equalToNumber', 'greaterThanNumber', 'greaterThanEqualNumber', 'lessThanNumber', 'lessThanEqualNumber'] as const;
export type NumberOp = (typeof NUMBER_OPS)[number];
export const isNumberOp = (op: string): op is NumberOp => (NUMBER_OPS as readonly string[]).includes(op);

export const SIMPLE_OPS: MatcherOp[] = ['equalTo', 'contains', 'doesNotContain', 'matches', 'doesNotMatch', 'absent', 'before', 'after', 'equalToDateTime'];
export const SIMPLE_OPS_V4: MatcherOp[] = [...SIMPLE_OPS, ...NUMBER_OPS];
export const BODY_OPS: MatcherOp[] = ['equalToJson', 'matchesJsonPath', 'matchesJsonSchema', 'equalTo', 'contains', 'doesNotContain', 'matches', 'doesNotMatch', 'equalToXml', 'matchesXPath', 'binaryEqualTo', 'absent'];

export const OP_LABEL: Record<string, string> = {
  equalTo: 'equals',
  contains: 'contains',
  doesNotContain: 'not contains',
  matches: 'regex',
  doesNotMatch: 'not regex',
  absent: 'is absent',
  equalToJson: 'JSON equals',
  matchesJsonPath: 'JSONPath',
  matchesJsonSchema: 'JSON schema',
  equalToXml: 'XML equals',
  matchesXPath: 'XPath',
  before: 'date before',
  after: 'date after',
  equalToDateTime: 'date equals',
  binaryEqualTo: 'binary (base64)',
  equalToNumber: 'number =',
  greaterThanNumber: 'number >',
  greaterThanEqualNumber: 'number ≥',
  lessThanNumber: 'number <',
  lessThanEqualNumber: 'number ≤',
  and: 'all of (and)',
  or: 'any of (or)',
  not: 'not',
  hasExactly: 'has exactly',
  includes: 'includes',
};

/** Identify the primary operator of a content pattern. */
export function opOf(p: ContentPattern | undefined): string {
  if (!p) return 'equalTo';
  for (const k of [...MATCHER_OPS, 'and', 'or', 'not', 'hasExactly', 'includes']) if (k in p) return k;
  return Object.keys(p)[0] ?? 'equalTo';
}

export function patternValue(p: ContentPattern | undefined): string {
  if (!p) return '';
  const op = opOf(p);
  const v = p[op];
  if (op === 'absent') return '';
  if (v === undefined || v === null) return '';
  if (typeof v === 'string') return v;
  return JSON.stringify(v, null, 2);
}

export function describePattern(p: ContentPattern | undefined): string {
  if (!p) return '';
  const op = opOf(p);
  if (op === 'absent') return 'absent';
  const v = patternValue(p);
  return `${OP_LABEL[op] ?? op} ${v.length > 60 ? v.slice(0, 57) + '…' : v}`;
}

export function sortStubs(stubs: StubMapping[], by: 'priority' | 'url' | 'name' | 'method' | 'recent'): StubMapping[] {
  const a = [...stubs];
  switch (by) {
    case 'url':
      return a.sort((x, y) => urlOf(x.request).localeCompare(urlOf(y.request)));
    case 'name':
      return a.sort((x, y) => displayName(x).localeCompare(displayName(y)));
    case 'method':
      return a.sort((x, y) => methodLabel(x.request.method).localeCompare(methodLabel(y.request.method)) || urlOf(x.request).localeCompare(urlOf(y.request)));
    case 'recent':
      return a.sort((x, y) => (y.insertionIndex ?? 0) - (x.insertionIndex ?? 0));
    default:
      // WireMock order: priority asc (default 5), then most recently added first.
      return a.sort((x, y) => (x.priority ?? 5) - (y.priority ?? 5) || (y.insertionIndex ?? 0) - (x.insertionIndex ?? 0));
  }
}

export function matchesQuery(s: StubMapping, q: string): boolean {
  if (!q) return true;
  const needle = q.toLowerCase();
  // Support simple field filters: method:GET status:404 tag:x folder:y is:proxy
  const tokens = needle.split(/\s+/).filter(Boolean);
  return tokens.every((t) => {
    const [k, ...rest] = t.split(':');
    const v = rest.join(':');
    if (rest.length && v) {
      const tr = traitsOf(s);
      switch (k) {
        case 'method':
        case 'm':
          // `method:put` also finds {oneOf:[PUT,…]} / {noneOf:[…]} stubs that accept PUT.
          return methodLabel(s.request.method).toLowerCase() === v || (v !== 'any' && !!methodSetOf(s.request.method) && methodAccepts(s.request.method, v));
        case 'status':
        case 's':
          return String(s.response.status ?? 200).startsWith(v);
        case 'tag':
          return tagsOf(s).some((x) => x.toLowerCase().includes(v));
        case 'folder':
          return folderOf(s).toLowerCase().includes(v);
        case 'scenario':
          return (s.scenarioName ?? '').toLowerCase().includes(v);
        case 'id':
          return stubId(s).toLowerCase().startsWith(v);
        case 'is':
          return (
            (v === 'proxy' && tr.proxy) ||
            (v === 'fault' && !!tr.fault) ||
            (v === 'delay' && tr.delay) ||
            (v === 'templated' && tr.templated) ||
            (v === 'webhook' && tr.webhook) ||
            (v === 'scenario' && tr.scenario) ||
            (v === 'persistent' && tr.persistent) ||
            (v === 'disabled' && tr.disabled) ||
            (v === 'file' && tr.file) ||
            (v === 'channel' && !!tr.channel) ||
            (v === 'websocket' && tr.channel === 'websocket') ||
            (v === 'sse' && tr.channel === 'sse')
          );
      }
    }
    const hay = [
      s.name,
      methodLabel(s.request.method),
      urlOf(s.request),
      stubId(s),
      String(s.response.status ?? ''),
      folderOf(s),
      s.scenarioName,
      wmui(s).description,
      ...tagsOf(s),
      s.response.proxyBaseUrl,
      s.response.bodyFileName,
    ]
      .filter(Boolean)
      .join(' ')
      .toLowerCase();
    return hay.includes(t);
  });
}

export function prettyJson(v: unknown): string {
  return JSON.stringify(v, null, 2);
}

export function tryParseJson<T = unknown>(s: string): { ok: true; value: T } | { ok: false; error: string } {
  try {
    return { ok: true, value: JSON.parse(s) as T };
  } catch (e) {
    return { ok: false, error: (e as Error).message };
  }
}

export function headerValue(h: Record<string, string | string[]> | undefined, name: string): string | undefined {
  if (!h) return undefined;
  const k = Object.keys(h).find((x) => x.toLowerCase() === name.toLowerCase());
  if (!k) return undefined;
  const v = h[k];
  return Array.isArray(v) ? v.join(', ') : v;
}

/** Build a folder tree from stubs for the sidebar. */
export interface FolderNode {
  name: string;
  path: string;
  children: Map<string, FolderNode>;
  stubs: StubMapping[];
  count: number;
}

export function buildFolderTree(stubs: StubMapping[]): FolderNode {
  const root: FolderNode = { name: '', path: '', children: new Map(), stubs: [], count: 0 };
  for (const s of stubs) {
    const f = folderOf(s);
    let node = root;
    node.count++;
    if (f) {
      let path = '';
      for (const part of f.split('/')) {
        path = path ? `${path}/${part}` : part;
        let child = node.children.get(part);
        if (!child) {
          child = { name: part, path, children: new Map(), stubs: [], count: 0 };
          node.children.set(part, child);
        }
        child.count++;
        node = child;
      }
    }
    node.stubs.push(s);
  }
  return root;
}
