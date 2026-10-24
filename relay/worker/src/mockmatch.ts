// Request matching for hosted mocks: a documented subset of WireMock's stub mappings
// (relay/README.md → "Hosted mocks → supported WireMock subset"). Anything outside the subset is
// reported by `compileStub` as a warning; unsupported *matchers* never match (fail closed), so a
// stub can't accidentally answer more requests than it does locally.

import subset from "./hosted-mock-subset.json";
import { queryJsonPath, JsonPathError } from "./jsonpath";

export const DEFAULT_PRIORITY = 5;
export const STARTED = "Started";

export interface MockRequest {
  method: string;
  /** Path + query exactly as received (without a path-style `/<name>` prefix). */
  url: string;
  /** Path without query. */
  path: string;
  query: [string, string][];
  headers: [string, string][];
  cookies: [string, string][];
  body: Uint8Array;
  bodyText: string;
  host: string;
}

export interface Warning {
  stub: string;
  feature: string;
  message: string;
}

type Matcher = (value: string | null) => boolean;

interface CompiledPattern {
  method: string | null;
  url: ((url: string, path: string) => Record<string, string> | null) | null;
  query: [string, Matcher][];
  headers: [string, Matcher][];
  cookies: [string, Matcher][];
  form: [string, Matcher][];
  body: ((req: MockRequest) => boolean)[];
  host: Matcher | null;
  basicAuth: string | null;
  /** An unsupported matcher was found: the stub never matches. */
  never: boolean;
}

export interface CompiledStub {
  id: string;
  name: string | null;
  seq: number;
  priority: number;
  pattern: CompiledPattern;
  scenarioName: string | null;
  requiredScenarioState: string | null;
  newScenarioState: string | null;
  /** The mapping's `response` object (rendered at request time). */
  response: Record<string, unknown>;
  templated: boolean;
  /** The original mapping JSON (journal + admin views). */
  json: Record<string, unknown>;
}

export interface MatchResult {
  stub: CompiledStub;
  pathParams: Record<string, string>;
}

const REQUEST_KEYS: ReadonlySet<string> = new Set(subset.requestKeys);
const RESPONSE_KEYS: ReadonlySet<string> = new Set(subset.responseKeys);
const MAPPING_KEYS: ReadonlySet<string> = new Set(subset.mappingKeys);
const DELAYS: ReadonlySet<string> = new Set(subset.delayDistributions);

function isObj(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

function fullRegex(src: string): RegExp {
  // WireMock uses Java's full-match semantics; inline flags like (?i) are translated.
  let flags = "";
  let s = src;
  const m = /^\(\?([imsux]+)\)/.exec(s);
  if (m) {
    flags = [...new Set(m[1]!.replace(/[xu]/g, ""))].join("");
    s = s.slice(m[0].length);
  }
  return new RegExp(`^(?:${s})$`, flags + "u");
}

// ---------- string value patterns ----------

class Unsupported extends Error {}

function compileValuePattern(p: unknown, where: string): Matcher {
  if (!isObj(p)) throw new Unsupported(`${where}: expected a pattern object`);
  const ci = p.caseInsensitive === true;
  const keys = Object.keys(p).filter((k) => !["caseInsensitive", "ignoreArrayOrder", "ignoreExtraElements", "enablePlaceholders"].includes(k));
  if (keys.length !== 1) {
    if (keys.length === 0) throw new Unsupported(`${where}: empty pattern`);
    throw new Unsupported(`${where}: one matcher per pattern (got ${keys.join(", ")})`);
  }
  const k = keys[0]!;
  const v = p[k];
  switch (k) {
    case "equalTo": {
      const want = String(v);
      return ci ? (x) => x !== null && x.toLowerCase() === want.toLowerCase() : (x) => x === want;
    }
    case "contains": {
      const want = String(v);
      return ci ? (x) => x !== null && x.toLowerCase().includes(want.toLowerCase()) : (x) => x !== null && x.includes(want);
    }
    case "doesNotContain": {
      const want = String(v);
      return (x) => x === null || !x.includes(want);
    }
    case "matches": {
      const re = safeRegex(String(v), where);
      return (x) => x !== null && re.test(x);
    }
    case "doesNotMatch": {
      const re = safeRegex(String(v), where);
      return (x) => x === null || !re.test(x);
    }
    case "absent":
      return v === false ? (x) => x !== null : (x) => x === null;
    case "anything":
      return () => true;
    case "equalToJson": {
      const want = typeof v === "string" ? parseJsonOrThrow(v, where) : v;
      const opts = { ignoreArrayOrder: p.ignoreArrayOrder === true, ignoreExtraElements: p.ignoreExtraElements === true };
      return (x) => {
        if (x === null) return false;
        let got: unknown;
        try {
          got = JSON.parse(x);
        } catch {
          return false;
        }
        return jsonEquals(want, got, opts);
      };
    }
    case "matchesJsonPath":
      return compileJsonPathPattern(v, where);
    case "binaryEqualTo": {
      const want = String(v);
      return (x) => x !== null && x === want; // compared against the base64 body by the caller
    }
    case "and":
    case "or": {
      if (!Array.isArray(v)) throw new Unsupported(`${where}: ${k} needs an array`);
      const ms = v.map((x, i) => compileValuePattern(x, `${where}.${k}[${i}]`));
      return k === "and" ? (x) => ms.every((m) => m(x)) : (x) => ms.some((m) => m(x));
    }
    case "not": {
      const m = compileValuePattern(v, `${where}.not`);
      return (x) => !m(x);
    }
    default:
      throw new Unsupported(`${where}: "${k}" isn't supported on hosted mocks`);
  }
}

function safeRegex(src: string, where: string): RegExp {
  try {
    return fullRegex(src);
  } catch {
    try {
      // Without the `u` flag some Java-style escapes are accepted.
      return new RegExp(`^(?:${src})$`);
    } catch {
      throw new Unsupported(`${where}: regex "${src}" isn't valid JavaScript`);
    }
  }
}

function parseJsonOrThrow(s: string, where: string): unknown {
  try {
    return JSON.parse(s);
  } catch {
    throw new Unsupported(`${where}: equalToJson isn't valid JSON`);
  }
}

function compileJsonPathPattern(v: unknown, where: string): Matcher {
  const expr = typeof v === "string" ? v : isObj(v) && typeof v.expression === "string" ? v.expression : null;
  if (expr === null) throw new Unsupported(`${where}: matchesJsonPath needs an expression`);
  try {
    queryJsonPath({}, expr);
  } catch (e) {
    if (e instanceof JsonPathError) throw new Unsupported(`${where}: JSONPath ${expr}: ${e.message}`);
    throw e;
  }
  let sub: Matcher | null = null;
  if (isObj(v)) {
    const rest = { ...v };
    delete rest.expression;
    if (Object.keys(rest).length) sub = compileValuePattern(rest, `${where}.matchesJsonPath`);
  }
  return (x) => {
    if (x === null) return false;
    let doc: unknown;
    try {
      doc = JSON.parse(x);
    } catch {
      return false;
    }
    const vals = queryJsonPath(doc, expr).filter((r) => r !== null && r !== undefined);
    if (!sub) return vals.length > 0 && !(vals.length === 1 && Array.isArray(vals[0]) && vals[0].length === 0);
    if (!vals.length) return sub(null);
    const candidates: string[] = [];
    if (vals.length === 1) {
      const one = vals[0];
      candidates.push(typeof one === "string" ? one : JSON.stringify(one));
      if (Array.isArray(one)) for (const e of one) candidates.push(typeof e === "string" ? e : JSON.stringify(e));
    } else {
      candidates.push(JSON.stringify(vals));
      for (const e of vals) candidates.push(typeof e === "string" ? e : JSON.stringify(e));
    }
    return candidates.some((c) => sub(c));
  };
}

// ---------- equalToJson with json-unit placeholders ----------

function placeholder(want: string, got: unknown): boolean | null {
  if (!want.startsWith("${json-unit.")) return null;
  if (want === "${json-unit.ignore}" || want === "${json-unit.any}") return true;
  if (want === "${json-unit.any-string}") return typeof got === "string";
  if (want === "${json-unit.any-number}") return typeof got === "number";
  if (want === "${json-unit.any-boolean}") return typeof got === "boolean";
  if (want.startsWith("${json-unit.regex}")) {
    try {
      return typeof got === "string" && fullRegex(want.slice("${json-unit.regex}".length)).test(got);
    } catch {
      return false;
    }
  }
  return null;
}

export function jsonEquals(want: unknown, got: unknown, o: { ignoreArrayOrder: boolean; ignoreExtraElements: boolean }): boolean {
  if (typeof want === "string") {
    const p = placeholder(want, got);
    if (p !== null) return p;
  }
  if (want === null || typeof want !== "object") return want === got;
  if (Array.isArray(want)) {
    if (!Array.isArray(got)) return false;
    if (!o.ignoreExtraElements && want.length !== got.length) return false;
    if (o.ignoreExtraElements && got.length < want.length) return false;
    if (!o.ignoreArrayOrder) return want.every((w, i) => jsonEquals(w, got[i], o));
    const used = new Set<number>();
    return want.every((w) => {
      for (let i = 0; i < got.length; i++) {
        if (!used.has(i) && jsonEquals(w, got[i], o)) {
          used.add(i);
          return true;
        }
      }
      return false;
    });
  }
  if (!isObj(got)) return false;
  const wk = Object.keys(want);
  const gk = Object.keys(got);
  if (!o.ignoreExtraElements && gk.length !== wk.length) return false;
  return wk.every((k) => Object.prototype.hasOwnProperty.call(got, k) && jsonEquals((want as Record<string, unknown>)[k], got[k], o));
}

// ---------- URL patterns ----------

function compileUrl(req: Record<string, unknown>): CompiledPattern["url"] {
  if (typeof req.url === "string") {
    const want = req.url;
    return (url) => (url === want ? {} : null);
  }
  if (typeof req.urlPath === "string") {
    const want = req.urlPath;
    return (_url, path) => (path === want ? {} : null);
  }
  if (typeof req.urlPattern === "string") {
    const re = safeRegex(req.urlPattern, "request.urlPattern");
    return (url) => (re.test(url) ? {} : null);
  }
  if (typeof req.urlPathPattern === "string") {
    const re = safeRegex(req.urlPathPattern, "request.urlPathPattern");
    return (_url, path) => (re.test(path) ? {} : null);
  }
  if (typeof req.urlPathTemplate === "string") {
    const tpl = req.urlPathTemplate.split("/");
    return (_url, path) => {
      const segs = path.split("/");
      if (segs.length !== tpl.length) return null;
      const vars: Record<string, string> = {};
      for (let i = 0; i < tpl.length; i++) {
        const t = tpl[i]!;
        const m = /^\{([^}]+)\}$/.exec(t);
        if (m) {
          if (!segs[i]) return null;
          try {
            vars[m[1]!] = decodeURIComponent(segs[i]!);
          } catch {
            vars[m[1]!] = segs[i]!;
          }
        } else if (t !== segs[i]) return null;
      }
      return vars;
    };
  }
  return null;
}

function compileMap(v: unknown, where: string): [string, Matcher][] {
  if (v === undefined || v === null) return [];
  if (!isObj(v)) throw new Unsupported(`${where}: expected an object`);
  return Object.entries(v).map(([name, p]) => [name, compileValuePattern(p, `${where}.${name}`)]);
}

function values(list: [string, string][], name: string, ci: boolean): string[] {
  const n = ci ? name.toLowerCase() : name;
  return list.filter(([k]) => (ci ? k.toLowerCase() : k) === n).map(([, v]) => v);
}

function anyValue(vals: string[], m: Matcher): boolean {
  if (!vals.length) return m(null);
  return vals.some((v) => m(v));
}

/**
 * Compiles one WireMock mapping. Unsupported request features make the stub never match;
 * unsupported response features are ignored. Both are reported in `warnings`.
 */
export function compileStub(json: Record<string, unknown>, seq: number, warnings: Warning[]): CompiledStub {
  const id = typeof json.id === "string" ? json.id : typeof json.uuid === "string" ? json.uuid : `stub-${seq}`;
  const name = typeof json.name === "string" ? json.name : null;
  const label = name ?? id;
  const warn = (feature: string, message: string) => warnings.push({ stub: label, feature, message });
  for (const k of Object.keys(json)) if (!MAPPING_KEYS.has(k)) warn(k, `"${k}" is ignored on hosted mocks`);
  if (json.postServeActions || json.serveEventListeners) warn("serveEventListeners", "webhooks / post-serve actions don't run on hosted mocks");

  const req = isObj(json.request) ? json.request : {};
  const pattern: CompiledPattern = { method: null, url: null, query: [], headers: [], cookies: [], form: [], body: [], host: null, basicAuth: null, never: false };
  const fail = (feature: string, message: string) => {
    warn(feature, `${message} — this stub never matches on hosted mocks`);
    pattern.never = true;
  };
  for (const k of Object.keys(req)) if (!REQUEST_KEYS.has(k)) fail(`request.${k}`, `request.${k} isn't supported`);
  try {
    const m = typeof req.method === "string" ? req.method.toUpperCase() : "ANY";
    pattern.method = m === "ANY" ? null : m;
    pattern.url = compileUrl(req);
    pattern.query = compileMap(req.queryParameters, "request.queryParameters");
    pattern.headers = compileMap(req.headers, "request.headers");
    pattern.cookies = compileMap(req.cookies, "request.cookies");
    pattern.form = compileMap(req.formParameters, "request.formParameters");
    if (req.host !== undefined) pattern.host = compileValuePattern(req.host, "request.host");
    if (isObj(req.basicAuthCredentials)) {
      const u = String(req.basicAuthCredentials.username ?? "");
      const p = String(req.basicAuthCredentials.password ?? "");
      pattern.basicAuth = `Basic ${bytesToB64(new TextEncoder().encode(`${u}:${p}`))}`;
    }
    if (req.bodyPatterns !== undefined) {
      if (!Array.isArray(req.bodyPatterns)) throw new Unsupported("request.bodyPatterns: expected an array");
      req.bodyPatterns.forEach((bp, i) => {
        const where = `request.bodyPatterns[${i}]`;
        if (isObj(bp) && "binaryEqualTo" in bp) {
          const want = String(bp.binaryEqualTo);
          pattern.body.push((r) => bytesToB64(r.body) === want);
          return;
        }
        const m = compileValuePattern(bp, where);
        pattern.body.push((r) => m(r.bodyText.length || r.body.byteLength ? r.bodyText : (isObj(bp) && "absent" in bp ? null : "")));
      });
    }
  } catch (e) {
    if (e instanceof Unsupported) fail("request", e.message);
    else throw e;
  }

  const response = isObj(json.response) ? json.response : {};
  for (const k of Object.keys(response)) {
    if (RESPONSE_KEYS.has(k)) continue;
    if (k === "proxyBaseUrl") warn("response.proxyBaseUrl", "proxying isn't available on hosted mocks (the stub answers 501)");
    else if (k === "fault") warn("response.fault", "faults can't be simulated at the edge (the stub answers 502 with x-devall-fault)");
    else if (k === "chunkedDribbleDelay") warn("response.chunkedDribbleDelay", "chunked dribble delays are ignored on hosted mocks");
    else warn(`response.${k}`, `response.${k} is ignored on hosted mocks`);
  }
  const transformers = Array.isArray(response.transformers) ? response.transformers.filter((t): t is string => typeof t === "string") : [];
  for (const t of transformers) if (t !== "response-template") warn("response.transformers", `transformer "${t}" isn't available on hosted mocks`);
  if (isObj(response.transformerParameters) && "dataSource" in response.transformerParameters) {
    warn("response.transformerParameters.dataSource", "data sources (mocks/data) aren't uploaded to hosted mocks");
  }
  const dd = response.delayDistribution;
  if (isObj(dd) && !DELAYS.has(String(dd.type))) warn("response.delayDistribution", `delay distribution "${String(dd.type)}" is ignored`);

  const priority = json.priority === undefined || json.priority === null ? DEFAULT_PRIORITY : Number(json.priority);
  return {
    id,
    name,
    seq,
    priority: Number.isFinite(priority) ? priority : DEFAULT_PRIORITY,
    pattern,
    scenarioName: typeof json.scenarioName === "string" ? json.scenarioName : null,
    requiredScenarioState: typeof json.requiredScenarioState === "string" ? json.requiredScenarioState : null,
    newScenarioState: typeof json.newScenarioState === "string" ? json.newScenarioState : null,
    response,
    templated: transformers.includes("response-template"),
    json,
  };
}

/** WireMock order: priority ascending, then the most recently added first. */
export function sortStubs(stubs: CompiledStub[]): CompiledStub[] {
  return [...stubs].sort((a, b) => a.priority - b.priority || b.seq - a.seq);
}

function matchesPattern(p: CompiledPattern, r: MockRequest): Record<string, string> | null {
  if (p.never) return null;
  if (p.method !== null && p.method !== r.method.toUpperCase()) return null;
  let params: Record<string, string> = {};
  if (p.url) {
    const got = p.url(r.url, r.path);
    if (!got) return null;
    params = got;
  }
  for (const [name, m] of p.query) if (!anyValue(values(r.query, name, false), m)) return null;
  for (const [name, m] of p.headers) if (!anyValue(values(r.headers, name, true), m)) return null;
  for (const [name, m] of p.cookies) if (!anyValue(values(r.cookies, name, false), m)) return null;
  if (p.form.length) {
    const form = parseForm(r.bodyText);
    for (const [name, m] of p.form) if (!anyValue(values(form, name, false), m)) return null;
  }
  if (p.host && !p.host(r.host)) return null;
  if (p.basicAuth !== null && values(r.headers, "authorization", true)[0] !== p.basicAuth) return null;
  for (const b of p.body) if (!b(r)) return null;
  return params;
}

/** First stub (in WireMock order) whose pattern and scenario state accept the request. */
export function findMatch(sorted: CompiledStub[], r: MockRequest, scenarioState: (name: string) => string): MatchResult | null {
  for (const s of sorted) {
    if (s.scenarioName && s.requiredScenarioState && scenarioState(s.scenarioName) !== s.requiredScenarioState) continue;
    const params = matchesPattern(s.pattern, r);
    if (params) return { stub: s, pathParams: params };
  }
  return null;
}

export function parseForm(body: string): [string, string][] {
  const out: [string, string][] = [];
  for (const [k, v] of new URLSearchParams(body)) out.push([k, v]);
  return out;
}

export function parseCookies(header: string | null): [string, string][] {
  if (!header) return [];
  return header
    .split(";")
    .map((c) => c.trim())
    .filter(Boolean)
    .map((c) => {
      const i = c.indexOf("=");
      return i < 0 ? ([c, ""] as [string, string]) : ([c.slice(0, i).trim(), c.slice(i + 1).trim()] as [string, string]);
    });
}

function bytesToB64(b: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < b.length; i += 0x8000) bin += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(bin);
}
