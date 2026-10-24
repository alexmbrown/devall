// MockDO — one published (hosted) mock: a project's WireMock stubs served from the edge while the
// laptop is off, at https://<name>.<mock domain>/… (or https://mock.<domain>/<name>/…).
//
// Storage is the object's SQLite: stubs (mapping JSON), small __files, scenario states and the last
// N journal entries. Matching/templating is the WireMock subset in mockmatch.ts / mocktemplate.ts.

import { DurableObject } from "cloudflare:workers";
import type { Env } from "./env";
import { base64ToBytes, bytesToBase64, clientIp, contentLength, NOINDEX, nullBodyStatus, readBodyCapped } from "./http";
import { compileStub, findMatch, parseCookies, sortStubs, STARTED, type CompiledStub, type MockRequest, type Warning } from "./mockmatch";
import { Bag, Multi, PathValue, renderTemplate, TemplateError } from "./mocktemplate";
import { ulid } from "./names";
import { limitsFor, type Plan } from "./plans";
import { dayKey, nextUtcMidnight, RateLimiter } from "./quota";

/** One file in __files may not exceed this (a SQLite row in a Durable Object is limited to 2 MB). */
export const MAX_FILE_BYTES = 1536 * 1024;
export const MAX_MAPPING_BYTES = 1024 * 1024;
/** Hosted mocks only accept small request bodies. */
export const MAX_REQUEST_BODY = 1024 * 1024;
/** Delays are capped: a sleeping request keeps the object (and its bill) awake. */
export const MAX_DELAY_MS = 10_000;
const JOURNAL_BODY = 8 * 1024;

export interface MockSettings {
  /** Answer CORS preflights and add `access-control-allow-origin: *` (WireMock's --enable-stub-cors). */
  cors: boolean;
  /** Template every response (WireMock's --global-response-templating). */
  globalTemplating: boolean;
}

export interface MockMeta {
  name: string;
  owner: string;
  plan: Plan;
  createdAt: number;
  updatedAt: number;
  stubs: number;
  bytes: number;
  settings: MockSettings;
  day: { key: string; count: number; rejected: number };
}

export interface PublishInput {
  uid: string;
  plan: Plan;
  name: string;
  mappings: unknown[];
  /** `__files` content, path → base64. */
  files: Record<string, string>;
  settings?: Partial<MockSettings>;
}

export type PublishResult =
  | { ok: true; stubs: number; bytes: number; files: number; warnings: Warning[]; createdAt: number; updatedAt: number }
  | { ok: false; status: number; code: string; message: string; warnings?: Warning[] };

export interface JournalEntry {
  id: string;
  at: string;
  method: string;
  url: string;
  headers: [string, string][];
  body: string;
  bodySize: number;
  clientIp: string | null;
  matched: boolean;
  stubId: string | null;
  stubName: string | null;
  status: number;
  durationMs: number;
}

export interface MockInfo {
  name: string;
  owner: string;
  stubs: number;
  bytes: number;
  createdAt: string;
  updatedAt: string;
  requestsToday: number;
  settings: MockSettings;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** Content types never served as active content from hosted plans (phishing / XSS). */
const ACTIVE_TYPES = /html|svg|javascript|ecmascript|xml-stylesheet/i;

function safePath(p: string): string | null {
  const s = p.replace(/\\/g, "/").replace(/^\/+/, "");
  if (!s || s.length > 300 || s.split("/").some((seg) => seg === ".." || seg === "." || seg === "")) return null;
  return s;
}

export class MockDO extends DurableObject<Env> {
  private sql: SqlStorage;
  private compiled: CompiledStub[] | null = null;
  private limiter: { perMinute: number; rl: RateLimiter } | null = null;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec(`CREATE TABLE IF NOT EXISTS stubs (seq INTEGER PRIMARY KEY, json TEXT NOT NULL)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY, body BLOB, size INTEGER NOT NULL)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS journal (seq INTEGER PRIMARY KEY AUTOINCREMENT, entry TEXT NOT NULL)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS scenarios (name TEXT PRIMARY KEY, state TEXT NOT NULL)`);
  }

  private meta(): Promise<MockMeta | undefined> {
    return this.ctx.storage.get<MockMeta>("meta");
  }

  // ---------- owner API (RPC from the Worker) ----------

  async publish(input: PublishInput): Promise<PublishResult> {
    const existing = await this.meta();
    if (existing && existing.owner !== input.uid) return { ok: false, status: 409, code: "name_taken", message: `"${input.name}" is published by another account.` };
    const limits = limitsFor(input.plan, this.env);
    if (!Array.isArray(input.mappings)) return { ok: false, status: 400, code: "bad_request", message: "mappings must be an array" };
    if (limits.mockMaxStubs !== null && input.mappings.length > limits.mockMaxStubs) {
      return { ok: false, status: 413, code: "too_many_stubs", message: `Your plan allows ${limits.mockMaxStubs} stubs per hosted mock (this one has ${input.mappings.length}).` };
    }
    const warnings: Warning[] = [];
    const rows: string[] = [];
    let bytes = 0;
    for (let i = 0; i < input.mappings.length; i++) {
      const m = input.mappings[i];
      if (!m || typeof m !== "object" || Array.isArray(m)) return { ok: false, status: 400, code: "bad_mapping", message: `mapping #${i + 1} is not an object` };
      const text = JSON.stringify(m);
      if (text.length > MAX_MAPPING_BYTES) return { ok: false, status: 413, code: "mapping_too_large", message: `mapping #${i + 1} is larger than ${MAX_MAPPING_BYTES} bytes` };
      compileStub(m as Record<string, unknown>, i, warnings);
      rows.push(text);
      bytes += enc.encode(text).byteLength;
    }
    const files: [string, Uint8Array][] = [];
    for (const [path, b64] of Object.entries(input.files ?? {})) {
      const p = safePath(path);
      if (!p || typeof b64 !== "string") return { ok: false, status: 400, code: "bad_file", message: `invalid file path ${path}` };
      let body: Uint8Array;
      try {
        body = base64ToBytes(b64);
      } catch {
        return { ok: false, status: 400, code: "bad_file", message: `${path} is not base64` };
      }
      if (body.byteLength > MAX_FILE_BYTES) return { ok: false, status: 413, code: "file_too_large", message: `${path} is larger than ${MAX_FILE_BYTES} bytes` };
      files.push([p, body]);
      bytes += body.byteLength;
    }
    if (limits.mockMaxBytes !== null && bytes > limits.mockMaxBytes) {
      return { ok: false, status: 413, code: "too_large", message: `Your plan allows ${limits.mockMaxBytes} bytes of stubs and files per hosted mock (this one has ${bytes}).` };
    }
    const now = Date.now();
    // Replace everything atomically (a publish is a full snapshot of the project's mocks).
    this.ctx.storage.transactionSync(() => {
      this.sql.exec("DELETE FROM stubs");
      this.sql.exec("DELETE FROM files");
      this.sql.exec("DELETE FROM scenarios");
      rows.forEach((json, i) => this.sql.exec("INSERT INTO stubs (seq, json) VALUES (?, ?)", i, json));
      for (const [p, body] of files) this.sql.exec("INSERT INTO files (path, body, size) VALUES (?, ?, ?)", p, body.byteLength ? body.slice().buffer : null, body.byteLength);
    });
    const meta: MockMeta = {
      name: input.name,
      owner: input.uid,
      plan: input.plan,
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
      stubs: rows.length,
      bytes,
      settings: { cors: input.settings?.cors ?? existing?.settings.cors ?? true, globalTemplating: input.settings?.globalTemplating ?? existing?.settings.globalTemplating ?? false },
      day: existing?.day ?? { key: dayKey(now), count: 0, rejected: 0 },
    };
    await this.ctx.storage.put("meta", meta);
    this.compiled = null;
    return { ok: true, stubs: rows.length, bytes, files: files.length, warnings, createdAt: meta.createdAt, updatedAt: now };
  }

  async unpublish(uid: string): Promise<boolean> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return false;
    this.compiled = null;
    await this.ctx.storage.deleteAll();
    return true;
  }

  async info(uid: string | null): Promise<MockInfo | null> {
    const meta = await this.meta();
    if (!meta || (uid !== null && meta.owner !== uid)) return null;
    return {
      name: meta.name,
      owner: meta.owner,
      stubs: meta.stubs,
      bytes: meta.bytes,
      createdAt: new Date(meta.createdAt).toISOString(),
      updatedAt: new Date(meta.updatedAt).toISOString(),
      requestsToday: meta.day.key === dayKey() ? meta.day.count : 0,
      settings: meta.settings,
    };
  }

  /** Newest first. */
  async journal(uid: string, limit: number): Promise<JournalEntry[] | null> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return null;
    const rows = this.sql.exec("SELECT entry FROM journal ORDER BY seq DESC LIMIT ?", Math.max(1, Math.min(1000, Math.trunc(limit) || 50))).toArray();
    return rows.map((r) => JSON.parse(String(r.entry)) as JournalEntry);
  }

  async clearJournal(uid: string): Promise<boolean> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return false;
    this.sql.exec("DELETE FROM journal");
    return true;
  }

  async resetScenarios(uid: string): Promise<boolean> {
    const meta = await this.meta();
    if (!meta || meta.owner !== uid) return false;
    this.sql.exec("DELETE FROM scenarios");
    return true;
  }

  // ---------- public side ----------

  private stubs(): CompiledStub[] {
    if (this.compiled) return this.compiled;
    const out: CompiledStub[] = [];
    const ignored: Warning[] = [];
    for (const row of this.sql.exec("SELECT seq, json FROM stubs ORDER BY seq").toArray()) {
      out.push(compileStub(JSON.parse(String(row.json)) as Record<string, unknown>, Number(row.seq), ignored));
    }
    this.compiled = sortStubs(out);
    return this.compiled;
  }

  private scenarioState(name: string): string {
    const row = this.sql.exec("SELECT state FROM scenarios WHERE name = ?", name).toArray()[0];
    return row ? String(row.state) : STARTED;
  }

  override async fetch(request: Request): Promise<Response> {
    const started = Date.now();
    const meta = await this.meta();
    if (!meta) return text(404, "No hosted mock is published at this address.");
    const limits = limitsFor(meta.plan, this.env);
    const ip = clientIp(request);
    if (limits.ipRequestsPerMinute !== null) {
      if (!this.limiter || this.limiter.perMinute !== limits.ipRequestsPerMinute) this.limiter = { perMinute: limits.ipRequestsPerMinute, rl: new RateLimiter(limits.ipRequestsPerMinute) };
      const key = ip ?? "unknown";
      if (!this.limiter.rl.allow(key, started)) return text(429, "Too many requests from this address.", { "retry-after": String(this.limiter.rl.retryAfter(key, started)) });
    }
    const today = dayKey(started);
    if (meta.day.key !== today) meta.day = { key: today, count: 0, rejected: 0 };
    if (limits.mockRequestsPerDay !== null && meta.day.count >= limits.mockRequestsPerDay) {
      meta.day.rejected += 1;
      await this.ctx.storage.put("meta", meta);
      return text(429, "This hosted mock reached its daily request limit.", { "retry-after": String(Math.ceil((nextUtcMidnight(started) - started) / 1000)) });
    }
    meta.day.count += 1;
    await this.ctx.storage.put("meta", meta);

    const url = new URL(request.url);
    const prefix = request.headers.get("x-devall-mock-prefix") ?? "";
    let path = url.pathname;
    if (prefix && path.startsWith(prefix)) path = path.slice(prefix.length) || "/";
    if (!path.startsWith("/")) path = `/${path}`;

    const len = contentLength(request.headers);
    if (len !== null && len > MAX_REQUEST_BODY) return text(413, `Hosted mocks accept bodies up to ${MAX_REQUEST_BODY} bytes.`);
    const body = await readBodyCapped(request.body, MAX_REQUEST_BODY);
    if (body === null) return text(413, `Hosted mocks accept bodies up to ${MAX_REQUEST_BODY} bytes.`);
    const headers: [string, string][] = [];
    for (const [k, v] of request.headers) {
      const n = k.toLowerCase();
      if (n.startsWith("cf-") || n.startsWith("x-devall-") || n === "cdn-loop" || n === "x-forwarded-proto" || n === "x-real-ip") continue;
      headers.push([n, v]);
    }
    const mreq: MockRequest = {
      method: request.method,
      url: path + url.search,
      path,
      query: [...url.searchParams.entries()],
      headers,
      cookies: parseCookies(request.headers.get("cookie")),
      body,
      bodyText: dec.decode(body),
      host: url.hostname,
    };
    const preflight = meta.settings.cors && request.method === "OPTIONS" && request.headers.has("access-control-request-method");
    const match = preflight ? null : findMatch(this.stubs(), mreq, (n) => this.scenarioState(n));
    let res: Response;
    if (preflight) {
      res = new Response(null, { status: 204, headers: corsHeaders(request) });
    } else if (!match) {
      res = text(404, `No stub matched ${request.method} ${mreq.url}.\nPublished stubs: ${meta.stubs}. Check the request journal in /dev/all.`);
    } else {
      const s = match.stub;
      if (s.scenarioName && s.newScenarioState) {
        this.sql.exec("INSERT INTO scenarios (name, state) VALUES (?, ?) ON CONFLICT(name) DO UPDATE SET state = excluded.state", s.scenarioName, s.newScenarioState);
      }
      res = await this.respond(s, mreq, match.pathParams, meta, request, `${url.origin}${prefix}`);
    }
    if (meta.settings.cors) for (const [k, v] of Object.entries(corsHeaders(request))) if (!res.headers.has(k)) res.headers.set(k, v);
    this.record(mreq, ip, match?.stub ?? null, res.status, Date.now() - started, limits.mockJournalSize);
    return this.finish(res, meta);
  }

  private finish(res: Response, meta: MockMeta): Response {
    res.headers.set("x-robots-tag", NOINDEX);
    res.headers.set("x-content-type-options", "nosniff");
    // Hosted mocks are for API clients: documents rendered from them can't run scripts or post forms.
    res.headers.set("content-security-policy", "sandbox; default-src 'none'");
    if (meta.plan !== "selfhost") {
      const ct = res.headers.get("content-type");
      if (ct && ACTIVE_TYPES.test(ct)) res.headers.set("content-type", "text/plain; charset=utf-8");
    }
    return res;
  }

  private async respond(s: CompiledStub, r: MockRequest, pathParams: Record<string, string>, meta: MockMeta, request: Request, baseUrl: string): Promise<Response> {
    const def = s.response;
    if (typeof def.proxyBaseUrl === "string") return text(501, "Proxying isn't available on hosted mocks.");
    const templated = s.templated || meta.settings.globalTemplating;
    const model = templated ? templateModel(r, pathParams, baseUrl, request, def.transformerParameters) : null;
    const render = (v: string): string => (model ? renderTemplate(v, model) : v);
    let status = typeof def.status === "number" ? def.status : Number(def.status ?? 200);
    if (!Number.isInteger(status) || status < 200 || status > 599) status = 200;
    const headers = new Headers();
    try {
      if (def.fault !== undefined) {
        return new Response(`Simulated fault ${String(def.fault)} (faults can't be reproduced at the edge).\n`, { status: 502, headers: { "x-devall-fault": String(def.fault), "content-type": "text/plain; charset=utf-8" } });
      }
      if (def.headers && typeof def.headers === "object" && !Array.isArray(def.headers)) {
        for (const [k, v] of Object.entries(def.headers as Record<string, unknown>)) {
          const vals = Array.isArray(v) ? v : [v];
          for (const x of vals) {
            try {
              headers.append(k, render(String(x)));
            } catch (e) {
              if (e instanceof TemplateError) throw e;
              // invalid header from the mapping: skip it
            }
          }
        }
      }
      let body: BodyInit | null = null;
      if (typeof def.body === "string") body = render(def.body);
      else if (def.jsonBody !== undefined && def.jsonBody !== null) body = render(JSON.stringify(def.jsonBody));
      else if (typeof def.base64Body === "string") body = base64ToBytes(def.base64Body);
      else if (typeof def.bodyFileName === "string") {
        const p = safePath(render(def.bodyFileName));
        const row = p ? this.sql.exec("SELECT body FROM files WHERE path = ?", p).toArray()[0] : undefined;
        if (!row) return text(500, `Body file ${def.bodyFileName} wasn't published (only small __files are uploaded).`);
        const bytes = row.body instanceof ArrayBuffer ? new Uint8Array(row.body) : new Uint8Array(0);
        body = templated ? render(dec.decode(bytes)) : bytes;
      }
      const delay = delayFor(def);
      if (delay > 0) await new Promise((res) => setTimeout(res, Math.min(delay, MAX_DELAY_MS)));
      if (nullBodyStatus(status) || request.method === "HEAD") body = null;
      return new Response(body, { status, statusText: typeof def.statusMessage === "string" ? def.statusMessage : undefined, headers });
    } catch (e) {
      if (e instanceof TemplateError) return text(500, `Template error in stub ${s.name ?? s.id}: ${e.message}`);
      throw e;
    }
  }

  private record(r: MockRequest, ip: string | null, stub: CompiledStub | null, status: number, durationMs: number, keep: number): void {
    const entry: JournalEntry = {
      id: ulid(),
      at: new Date().toISOString(),
      method: r.method,
      url: r.url,
      headers: r.headers.slice(0, 60),
      body: r.bodyText.slice(0, JOURNAL_BODY),
      bodySize: r.body.byteLength,
      clientIp: ip,
      matched: !!stub,
      stubId: stub?.id ?? null,
      stubName: stub?.name ?? null,
      status,
      durationMs,
    };
    this.sql.exec("INSERT INTO journal (entry) VALUES (?)", JSON.stringify(entry));
    this.sql.exec("DELETE FROM journal WHERE seq <= (SELECT MAX(seq) FROM journal) - ?", Math.max(1, keep));
  }
}

function text(status: number, message: string, headers: Record<string, string> = {}): Response {
  return new Response(`${message}\n`, { status, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", "x-robots-tag": NOINDEX, ...headers } });
}

function corsHeaders(request: Request): Record<string, string> {
  return {
    "access-control-allow-origin": request.headers.get("origin") ?? "*",
    "access-control-allow-methods": "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD",
    "access-control-allow-headers": request.headers.get("access-control-request-headers") ?? "*",
    "access-control-max-age": "600",
    vary: "Origin",
  };
}

function delayFor(def: Record<string, unknown>): number {
  if (typeof def.fixedDelayMilliseconds === "number") return Math.max(0, def.fixedDelayMilliseconds);
  const d = def.delayDistribution;
  if (!d || typeof d !== "object") return 0;
  const dd = d as Record<string, unknown>;
  if (dd.type === "fixed") return Math.max(0, Number(dd.milliseconds ?? 0));
  if (dd.type === "uniform") {
    const lo = Number(dd.lower ?? 0);
    const hi = Number(dd.upper ?? lo);
    return Math.max(0, lo + Math.random() * Math.max(0, hi - lo));
  }
  if (dd.type === "lognormal") {
    // Box–Muller standard normal → median * exp(sigma * z)
    const z = Math.sqrt(-2 * Math.log(Math.max(1e-12, Math.random()))) * Math.cos(2 * Math.PI * Math.random());
    return Math.max(0, Number(dd.median ?? 0) * Math.exp(Number(dd.sigma ?? 0.1) * z));
  }
  return 0;
}

/** WireMock's `request` model (+ `parameters`) for templates. */
export function templateModel(r: MockRequest, pathParams: Record<string, string>, baseUrl: string, request: Request, parameters: unknown): Record<string, unknown> {
  const segments = r.path.split("/").filter(Boolean).map((s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  });
  const query: Record<string, Multi> = {};
  for (const [k, v] of r.query) (query[k] ??= new Multi([])).values.push(v);
  const cookies: Record<string, Multi> = {};
  for (const [k, v] of r.cookies) (cookies[k] ??= new Multi([])).values.push(v);
  const url = new URL(request.url);
  return {
    request: {
      id: ulid(),
      url: r.url,
      path: new PathValue(r.path, segments, pathParams),
      pathSegments: segments,
      query,
      headers: new Bag(r.headers, true),
      cookies,
      method: r.method,
      body: r.bodyText,
      bodyAsBase64: bytesToBase64(r.body),
      baseUrl,
      host: url.hostname,
      port: url.port || (url.protocol === "https:" ? "443" : "80"),
      scheme: url.protocol.replace(/:$/, ""),
      clientIp: clientIp(request),
    },
    parameters: parameters && typeof parameters === "object" ? parameters : {},
  };
}
