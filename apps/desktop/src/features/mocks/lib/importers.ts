import { unzip, zipSync, strToU8, strFromU8 } from './zip';
import { parseYaml } from './yaml';
import type { StubMapping, ContentPattern, MessageStubMapping } from './types';
import { harToStubs, type Har } from './har';
import { requestToStub, DEFAULT_TO_STUB } from './request-to-stub';
import { folderOf, displayName, stubId } from './stub-utils';

function safeFileName(s: string): string {
  return s.replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 80) || 'stub';
}

export type ImportKind = 'stubs' | 'zip' | 'har' | 'curl' | 'openapi' | 'postman' | 'projects' | 'snapshot';

export interface ImportResult {
  kind: ImportKind;
  source: string;
  stubs: StubMapping[];
  files: Record<string, Uint8Array>;
  warnings: string[];
  projects?: unknown[];
  /** WireMock 4 message stubs (from a snapshot or a Messages page export). */
  messageStubs?: MessageStubMapping[];
}

export const SNAPSHOT_KIND = 'wiremock-ui/server-snapshot';

function isStub(x: unknown): x is StubMapping {
  return !!x && typeof x === 'object' && ('request' in (x as object) || 'response' in (x as object));
}

function stubsFromJson(data: unknown): StubMapping[] | undefined {
  if (Array.isArray(data) && data.every(isStub)) return data;
  if (data && typeof data === 'object') {
    const o = data as Record<string, unknown>;
    if (Array.isArray(o.mappings)) return (o.mappings as unknown[]).filter(isStub) as StubMapping[];
    if (isStub(o)) return [o];
  }
  return undefined;
}

/** `yaml` parses YAML text; the desktop app passes the Rust parser (full YAML 1.2). */
export async function importFile(file: File, yaml?: (text: string) => Promise<unknown>): Promise<ImportResult> {
  const name = file.name.toLowerCase();
  if (name.endsWith('.zip')) return importZip(new Uint8Array(await file.arrayBuffer()), file.name);
  const text = await file.text();
  return importTextAsync(text, file.name, yaml);
}

/** Like `importText`, but parses YAML with `yaml` when given. */
export async function importTextAsync(text: string, source = 'pasted text', yaml?: (text: string) => Promise<unknown>): Promise<ImportResult> {
  const trimmed = text.trim();
  if (!yaml || /^curl\s/i.test(trimmed)) return importText(text, source);
  try {
    JSON.parse(trimmed);
    return importText(text, source);
  } catch {
    let data: unknown;
    try {
      data = await yaml(trimmed);
    } catch (e) {
      throw new Error(`Not JSON, YAML or a cURL command: ${(e as Error).message}`);
    }
    return importText(JSON.stringify(data ?? null), source);
  }
}

export function importText(text: string, source = 'pasted text'): ImportResult {
  const base: ImportResult = { kind: 'stubs', source, stubs: [], files: {}, warnings: [] };
  const trimmed = text.trim();
  if (/^curl\s/i.test(trimmed)) {
    const stubs = trimmed
      .split(/\n(?=\s*curl\s)/i)
      .map((c) => curlToStub(c))
      .filter(Boolean) as StubMapping[];
    return { ...base, kind: 'curl', stubs };
  }
  let data: unknown;
  try {
    data = JSON.parse(trimmed);
  } catch {
    try {
      data = parseYaml(trimmed);
    } catch (e) {
      throw new Error(`Not JSON, YAML or a cURL command: ${(e as Error).message}`);
    }
  }
  const o = (data ?? {}) as Record<string, unknown>;
  if (o.kind === SNAPSHOT_KIND) {
    const files: Record<string, Uint8Array> = {};
    for (const [p, b64] of Object.entries((o.files as Record<string, string>) ?? {})) files[p] = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    return { ...base, kind: 'snapshot', stubs: (o.mappings as StubMapping[]) ?? [], files, messageStubs: (o.messageMappings as MessageStubMapping[] | undefined) ?? undefined };
  }
  if (Array.isArray(o.messageMappings) && !o.mappings) return { ...base, kind: 'stubs', messageStubs: o.messageMappings as MessageStubMapping[] };
  if (o.kind === 'wiremock-ui/projects' || (Array.isArray(o.projects) && !o.mappings)) return { ...base, kind: 'projects', projects: o.projects as unknown[] };
  if ((o.log as Har['log'])?.entries) return { ...base, kind: 'har', stubs: harToStubs(o as unknown as Har) };
  if (o.openapi || o.swagger) {
    const r = openApiToStubs(o);
    return { ...base, kind: 'openapi', stubs: r.stubs, warnings: r.warnings };
  }
  if ((o.info as { schema?: string })?.schema?.includes('postman') || (Array.isArray(o.item) && o.info)) return { ...base, kind: 'postman', ...postmanToStubs(o) };
  const stubs = stubsFromJson(data);
  if (stubs) return { ...base, stubs };
  throw new Error('Unrecognised file. Supported: WireMock stub JSON, {mappings:[]}, ZIP (mappings/ + __files/), HAR, OpenAPI/Swagger, Postman collection, cURL, WireMock UI exports.');
}

export async function importZip(bytes: Uint8Array, source: string): Promise<ImportResult> {
  const entries = await unzip(bytes);
  const out: ImportResult = { kind: 'zip', source, stubs: [], files: {}, warnings: [] };
  for (const [path, data] of Object.entries(entries)) {
    if (path.endsWith('/')) continue;
    const norm = path.replace(/\\/g, '/');
    const mi = norm.search(/(^|\/)mappings\//);
    const fi = norm.search(/(^|\/)__files\//);
    if (fi >= 0) {
      out.files[norm.slice(norm.indexOf('__files/') + 8)] = data;
    } else if (mi >= 0 && norm.endsWith('.json')) {
      try {
        const stubs = stubsFromJson(JSON.parse(strFromU8(data)));
        if (stubs) out.stubs.push(...stubs);
      } catch (e) {
        out.warnings.push(`${norm}: ${(e as Error).message}`);
      }
    } else if (norm.endsWith('.json')) {
      try {
        const stubs = stubsFromJson(JSON.parse(strFromU8(data)));
        if (stubs) out.stubs.push(...stubs);
      } catch {
        /* not a stub file */
      }
    }
  }
  return out;
}

/** WireMock root-dir layout: mappings/<folder>/<name>.json + __files/… — usable with --root-dir. */
export function buildZip(stubs: StubMapping[], files: Record<string, Uint8Array>): Uint8Array {
  const tree: Record<string, Uint8Array> = {};
  const used = new Set<string>();
  for (const s of stubs) {
    const folder = folderOf(s);
    let base = `${safeFileName(displayName(s)).toLowerCase()}-${stubId(s)?.slice(0, 8) ?? Math.random().toString(36).slice(2, 10)}`;
    while (used.has(`${folder}/${base}`)) base += '_';
    used.add(`${folder}/${base}`);
    tree[`mappings/${folder ? folder + '/' : ''}${base}.json`] = strToU8(JSON.stringify(s, null, 2));
  }
  for (const [p, b] of Object.entries(files)) tree[`__files/${p}`] = b;
  return zipSync(tree);
}

// ---------------------------------------------------------------- cURL

function shellSplit(cmd: string): string[] {
  const s = cmd.replace(/\\\r?\n/g, ' ').replace(/\^\r?\n/g, ' ');
  const out: string[] = [];
  let cur = '';
  let q: string | null = null;
  let had = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === q) q = null;
      else if (c === '\\' && q === '"' && i + 1 < s.length) cur += s[++i];
      else cur += c;
      continue;
    }
    if (c === "'" || c === '"') {
      q = c;
      had = true;
    } else if (c === '$' && s[i + 1] === "'") {
      // ANSI-C quoting $'..'
      i++;
      let j = i + 1;
      while (j < s.length && s[j] !== "'") {
        if (s[j] === '\\' && s[j + 1] === 'n') {
          cur += '\n';
          j += 2;
          continue;
        }
        if (s[j] === '\\') j++;
        cur += s[j++];
      }
      i = j;
      had = true;
    } else if (/\s/.test(c)) {
      if (cur || had) out.push(cur);
      cur = '';
      had = false;
    } else if (c === '\\' && i + 1 < s.length) cur += s[++i];
    else cur += c;
  }
  if (cur || had) out.push(cur);
  return out;
}

export function curlToStub(cmd: string): StubMapping | undefined {
  const t = shellSplit(cmd.trim());
  if (!t.length || !/curl/i.test(t[0])) return undefined;
  let method: string | undefined;
  let url = '';
  const headers: Record<string, string> = {};
  const data: string[] = [];
  let get = false;
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    const next = () => t[++i] ?? '';
    if (a === '-X' || a === '--request') method = next().toUpperCase();
    else if (a.startsWith('-X') && a.length > 2) method = a.slice(2).toUpperCase();
    else if (a === '-H' || a === '--header') {
      const h = next();
      const idx = h.indexOf(':');
      if (idx > 0) headers[h.slice(0, idx).trim()] = h.slice(idx + 1).trim();
    } else if (['-d', '--data', '--data-raw', '--data-binary', '--data-ascii'].includes(a)) data.push(next());
    else if (a === '--data-urlencode') data.push(next());
    else if (a === '--json') {
      data.push(next());
      headers['Content-Type'] ??= 'application/json';
      headers.Accept ??= 'application/json';
    } else if (a === '-u' || a === '--user') headers.Authorization = `Basic ${btoa(next())}`;
    else if (a === '-b' || a === '--cookie') headers.Cookie = next();
    else if (a === '-A' || a === '--user-agent') headers['User-Agent'] = next();
    else if (a === '-e' || a === '--referer') headers.Referer = next();
    else if (a === '--url') url = next();
    else if (a === '-G' || a === '--get') get = true;
    else if (['-o', '--output', '-w', '--write-out', '-m', '--max-time', '--connect-timeout', '-x', '--proxy', '--cacert', '--cert', '-F', '--form'].includes(a)) next();
    else if (a.startsWith('-')) continue;
    else if (!url) url = a;
  }
  if (!url) return undefined;
  let u: URL;
  try {
    u = new URL(/^https?:\/\//.test(url) ? url : `http://${url}`);
  } catch {
    return undefined;
  }
  let body = data.length ? data.join('&') : undefined;
  if (get && body) {
    u.search = u.search ? `${u.search}&${body}` : `?${body}`;
    body = undefined;
  }
  const m = method ?? (body ? 'POST' : 'GET');
  if (body && !Object.keys(headers).some((h) => h.toLowerCase() === 'content-type')) headers['Content-Type'] = 'application/x-www-form-urlencoded';
  const ctKey = Object.keys(headers).find((h) => h.toLowerCase() === 'content-type');
  const stub = requestToStub(
    { method: m, url: u.pathname + u.search, headers, body },
    undefined,
    { ...DEFAULT_TO_STUB, headers: ctKey && body ? [ctKey.toLowerCase()] : [], useResponse: false },
  );
  stub.name = `${m} ${u.pathname}`;
  if (headers[ctKey ?? ''] && stub.request.headers?.[ctKey!]) stub.request.headers[ctKey!] = { contains: headers[ctKey!].split(';')[0] };
  return stub;
}

// ---------------------------------------------------------------- OpenAPI

type Obj = Record<string, unknown>;

function resolveRef(doc: Obj, ref: string): unknown {
  if (!ref.startsWith('#/')) return undefined;
  return ref
    .slice(2)
    .split('/')
    .reduce<unknown>((o, k) => (o as Obj | undefined)?.[k.replace(/~1/g, '/').replace(/~0/g, '~')], doc);
}

function deref(doc: Obj, v: unknown, depth = 0): unknown {
  if (depth > 12 || !v || typeof v !== 'object') return v;
  const o = v as Obj;
  if (typeof o.$ref === 'string') return deref(doc, resolveRef(doc, o.$ref), depth + 1);
  return o;
}

/** Generate a plausible example from a JSON schema. */
export function sampleFromSchema(doc: Obj, schema: unknown, depth = 0): unknown {
  const s = deref(doc, schema) as Obj | undefined;
  if (!s || depth > 8) return null;
  if (s.example !== undefined) return s.example;
  if (Array.isArray(s.examples) && s.examples.length) return s.examples[0];
  if (s.default !== undefined) return s.default;
  if (Array.isArray(s.enum) && s.enum.length) return s.enum[0];
  if (s.const !== undefined) return s.const;
  for (const k of ['allOf', 'oneOf', 'anyOf'] as const) {
    if (Array.isArray(s[k])) {
      if (k === 'allOf') return Object.assign({}, ...(s[k] as unknown[]).map((x) => sampleFromSchema(doc, x, depth + 1) as Obj));
      return sampleFromSchema(doc, (s[k] as unknown[])[0], depth + 1);
    }
  }
  const type = Array.isArray(s.type) ? s.type[0] : s.type;
  if (type === 'object' || s.properties) {
    const out: Obj = {};
    for (const [k, v] of Object.entries((s.properties as Obj) ?? {})) out[k] = sampleFromSchema(doc, v, depth + 1);
    return out;
  }
  if (type === 'array') return [sampleFromSchema(doc, s.items, depth + 1)];
  if (type === 'integer') return typeof s.minimum === 'number' ? s.minimum : 1;
  if (type === 'number') return 1.5;
  if (type === 'boolean') return true;
  if (type === 'string') {
    const f = s.format;
    if (f === 'date-time') return '2026-01-01T12:00:00Z';
    if (f === 'date') return '2026-01-01';
    if (f === 'uuid') return '3fa85f64-5717-4562-b3fc-2c963f66afa6';
    if (f === 'email') return 'user@example.com';
    if (f === 'uri' || f === 'url') return 'https://example.com';
    return 'string';
  }
  return null;
}

export function openApiToStubs(doc: Obj): { stubs: StubMapping[]; warnings: string[] } {
  const stubs: StubMapping[] = [];
  const warnings: string[] = [];
  const swagger2 = !!doc.swagger;
  let basePath = '';
  if (swagger2) basePath = (doc.basePath as string) ?? '';
  else {
    const server = (doc.servers as { url: string }[] | undefined)?.[0]?.url;
    if (server) {
      try {
        basePath = new URL(server, 'http://x').pathname.replace(/\/$/, '');
      } catch {
        /* ignore */
      }
    }
  }
  const title = ((doc.info as Obj)?.title as string) ?? 'API';
  for (const [path, item] of Object.entries((doc.paths as Obj) ?? {})) {
    const pathItem = deref(doc, item) as Obj;
    for (const method of ['get', 'post', 'put', 'patch', 'delete', 'head', 'options']) {
      const op = pathItem[method] as Obj | undefined;
      if (!op) continue;
      const responses = (op.responses as Obj) ?? {};
      const code = Object.keys(responses).find((c) => /^2\d\d$/.test(c)) ?? Object.keys(responses).find((c) => c === 'default') ?? '200';
      const resp = deref(doc, responses[code]) as Obj | undefined;
      let body: unknown;
      let contentType = 'application/json';
      if (swagger2) {
        const ex = (resp?.examples as Obj | undefined)?.['application/json'];
        body = ex ?? (resp?.schema ? sampleFromSchema(doc, resp.schema) : undefined);
      } else {
        const content = (resp?.content as Obj) ?? {};
        contentType = Object.keys(content).find((c) => c.includes('json')) ?? Object.keys(content)[0] ?? 'application/json';
        const media = content[contentType] as Obj | undefined;
        if (media) {
          if (media.example !== undefined) body = media.example;
          else if (media.examples && typeof media.examples === 'object') body = (deref(doc, Object.values(media.examples as Obj)[0]) as Obj)?.value;
          else if (media.schema) body = sampleFromSchema(doc, media.schema);
        }
      }
      const hasParams = /\{[^}]+\}/.test(path);
      const params = [...((pathItem.parameters as unknown[]) ?? []), ...((op.parameters as unknown[]) ?? [])].map((p) => deref(doc, p) as Obj);
      const queryParameters: Record<string, ContentPattern> = {};
      for (const p of params) if (p.in === 'query' && p.required) queryParameters[p.name as string] = { matches: '.+' };
      const tag = (op.tags as string[] | undefined)?.[0];
      const status = code === 'default' ? 200 : Number(code);
      const stub: StubMapping = {
        name: (op.summary as string) || (op.operationId as string) || `${method.toUpperCase()} ${path}`,
        request: {
          method: method.toUpperCase(),
          ...(hasParams ? { urlPathTemplate: basePath + path } : { urlPath: basePath + path }),
          ...(Object.keys(queryParameters).length ? { queryParameters } : {}),
        },
        response: {
          status,
          ...(body !== undefined && status !== 204
            ? contentType.includes('json')
              ? { headers: { 'Content-Type': contentType }, jsonBody: body }
              : { headers: { 'Content-Type': contentType }, body: typeof body === 'string' ? body : JSON.stringify(body) }
            : {}),
        },
        metadata: { 'wiremock-gui': { folder: `/${safeFolder(title)}${tag ? `/${safeFolder(tag)}` : ''}` }, wmui: { description: (op.description as string) || undefined, tags: ['openapi'] } },
      };
      if (op.operationId) (stub.metadata!.wmui as Obj).operationId = op.operationId;
      stubs.push(stub);
    }
  }
  if (!stubs.length) warnings.push('No operations found in the spec.');
  return { stubs, warnings };
}

function safeFolder(s: string) {
  return s.replace(/[/\\]+/g, '-').trim();
}

// ---------------------------------------------------------------- Postman

export function postmanToStubs(col: Obj): { stubs: StubMapping[]; warnings: string[] } {
  const stubs: StubMapping[] = [];
  const warnings: string[] = [];
  const walk = (items: unknown[], folder: string) => {
    for (const it of items as Obj[]) {
      if (Array.isArray(it.item)) {
        walk(it.item, `${folder}/${safeFolder(String(it.name ?? 'folder'))}`);
        continue;
      }
      const req = it.request as Obj | string | undefined;
      if (!req) continue;
      const r = typeof req === 'string' ? { method: 'GET', url: req } : req;
      const rawUrl = typeof r.url === 'string' ? r.url : ((r.url as Obj)?.raw as string) ?? '';
      const path = rawUrl.replace(/^\{\{[^}]+\}\}/, '').replace(/^https?:\/\/[^/]+/, '') || '/';
      const pathTemplate = path.split('?')[0].replace(/:([A-Za-z_]\w*)/g, '{$1}').replace(/\{\{([^}]+)\}\}/g, '{$1}');
      const responses = (it.response as Obj[] | undefined) ?? [];
      const examples = responses.length ? responses : [undefined];
      for (const ex of examples) {
        const status = (ex?.code as number) ?? 200;
        const headers: Record<string, string> = {};
        for (const h of (ex?.header as { key: string; value: string }[]) ?? []) if (!/content-length|date|transfer-encoding/i.test(h.key)) headers[h.key] = h.value;
        const body = ex?.body as string | undefined;
        let jsonBody: unknown;
        try {
          jsonBody = body ? JSON.parse(body) : undefined;
        } catch {
          jsonBody = undefined;
        }
        stubs.push({
          name: `${String(it.name ?? '')}${ex?.name ? ` — ${ex.name}` : ''}`,
          request: { method: String(r.method ?? 'GET').toUpperCase(), ...(/\{/.test(pathTemplate) ? { urlPathTemplate: pathTemplate } : { urlPath: pathTemplate }) },
          response: { status, ...(Object.keys(headers).length ? { headers } : {}), ...(jsonBody !== undefined ? { jsonBody } : body ? { body } : {}) },
          metadata: { 'wiremock-gui': { folder: folder || '/postman' } },
        });
      }
      if (!responses.length) warnings.push(`"${it.name}" has no saved example response — created a 200 placeholder.`);
    }
  };
  walk((col.item as unknown[]) ?? [], `/${safeFolder(String((col.info as Obj)?.name ?? 'postman'))}`);
  return { stubs, warnings };
}
