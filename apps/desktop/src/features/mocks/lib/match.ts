/**
 * Best-effort, client-side re-implementation of WireMock request matching.
 * WireMock's admin API only reports a near-miss *distance*, so this produces
 * the per-field explanation ("header X expected …, got …") shown in the UI.
 * Anything we cannot evaluate faithfully is reported as `unknown`.
 */
import type { ContentPattern, LoggedRequest, MatchResult, RequestPattern, ServerFieldResult } from './types';
import { evaluate, isDefinite, JsonPathUnsupported } from './jsonpath';
import { opOf, OP_LABEL, URL_KEY_LABEL, urlKeyOf, DISABLED_HEADER, isNumberOp, methodAccepts, methodLabel, methodSetOf } from './stub-utils';

export type Verdict = 'match' | 'nomatch' | 'unknown';

export interface FieldResult {
  field: string;
  expected: string;
  actual: string;
  verdict: Verdict;
  note?: string;
}

export interface MatchReport {
  verdict: Verdict;
  fields: FieldResult[];
}

const combine = (vs: Verdict[]): Verdict => (vs.includes('nomatch') ? 'nomatch' : vs.includes('unknown') ? 'unknown' : 'match');

// ---------------------------------------------------------------- regex

export function javaRegex(re: string): RegExp {
  let flags = 's';
  let src = re;
  const m = /^\(\?([imsux]+)\)/.exec(src);
  if (m) {
    src = src.slice(m[0].length);
    if (m[1].includes('i')) flags += 'i';
    if (m[1].includes('m')) flags += 'm';
  }
  // Java possessive quantifiers / atomic groups → closest JS equivalent.
  src = src.replace(/([*+?}])\+/g, '$1').replace(/\(\?>/g, '(?:');
  return new RegExp(`^(?:${src})$`, flags);
}

// ---------------------------------------------------------------- dates

function parseExpectedDate(s: string): Date | undefined {
  const t = s.trim();
  const rel = /^now\s*(?:([+-])\s*(\d+)\s*(second|minute|hour|day|month|year)s?)?$/i.exec(t);
  if (rel) {
    const d = new Date();
    if (rel[1]) {
      const n = Number(rel[2]) * (rel[1] === '-' ? -1 : 1);
      const unit = rel[3].toLowerCase();
      if (unit === 'second') d.setSeconds(d.getSeconds() + n);
      if (unit === 'minute') d.setMinutes(d.getMinutes() + n);
      if (unit === 'hour') d.setHours(d.getHours() + n);
      if (unit === 'day') d.setDate(d.getDate() + n);
      if (unit === 'month') d.setMonth(d.getMonth() + n);
      if (unit === 'year') d.setFullYear(d.getFullYear() + n);
    }
    return d;
  }
  const d = new Date(t);
  return Number.isNaN(d.getTime()) ? undefined : d;
}

// ---------------------------------------------------------------- numbers (WireMock 4)

// Java's Double.parseDouble grammar (decimal form): optional sign, digits with an
// optional fraction or a bare fraction, optional exponent, optional f/F/d/D suffix.
const JAVA_DOUBLE = /^[+-]?(?:NaN|Infinity|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?[fFdD]?)$/;
const JAVA_HEX_DOUBLE = /^[+-]?0[xX](?:[0-9a-fA-F]+\.?[0-9a-fA-F]*|\.[0-9a-fA-F]+)[pP][+-]?\d+[fFdD]?$/;

/**
 * Parse like `Double.parseDouble`, which WireMock 4's number matchers use:
 * leading/trailing whitespace and control characters are ignored, `1e3`, `+6`,
 * `6d`, `Infinity` are numbers; `0x10`, `1_000`, `6abc` and `""` are not.
 * Returns `null` for hexadecimal floating point (valid Java, not evaluated here).
 */
export function parseJavaDouble(raw: string): number | undefined | null {
  // Java's trim() strips every char <= U+0020.
  // eslint-disable-next-line no-control-regex
  const t = raw.replace(/^[\u0000-\u0020]+|[\u0000-\u0020]+$/g, '');
  if (JAVA_DOUBLE.test(t)) return Number(t.replace(/[fFdD]$/, ''));
  return JAVA_HEX_DOUBLE.test(t) ? null : undefined;
}

const NUMBER_TEST: Record<string, (actual: number, expected: number) => boolean> = {
  equalToNumber: (a, e) => a === e,
  greaterThanNumber: (a, e) => a > e,
  greaterThanEqualNumber: (a, e) => a >= e,
  lessThanNumber: (a, e) => a < e,
  lessThanEqualNumber: (a, e) => a <= e,
};

// ---------------------------------------------------------------- JSON equality

const PH = /^\$\{json-unit\.([a-z-]+)\}(.*)$/s;

function jsonEq(expected: unknown, actual: unknown, opts: { arr: boolean; extra: boolean }): boolean {
  if (typeof expected === 'string') {
    const m = PH.exec(expected);
    if (m) {
      switch (m[1]) {
        case 'ignore':
        case 'ignore-element':
          return true;
        case 'any-string':
          return typeof actual === 'string';
        case 'any-number':
          return typeof actual === 'number';
        case 'any-boolean':
          return typeof actual === 'boolean';
        case 'regex':
          try {
            return typeof actual === 'string' || typeof actual === 'number' ? javaRegex(m[2]).test(String(actual)) : false;
          } catch {
            return false;
          }
        case 'matches':
          return true; // custom matcher; cannot evaluate
      }
    }
  }
  if (expected === null || typeof expected !== 'object') return expected === actual;
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) return false;
    if (!opts.extra && expected.length !== actual.length) return false;
    if (opts.extra && actual.length < expected.length) return false;
    if (!opts.arr) return expected.every((e, i) => jsonEq(e, actual[i], opts));
    const used = new Set<number>();
    return expected.every((e) => {
      const i = actual.findIndex((a, j) => !used.has(j) && jsonEq(e, a, opts));
      if (i < 0) return false;
      used.add(i);
      return true;
    });
  }
  if (!actual || typeof actual !== 'object' || Array.isArray(actual)) return false;
  const ek = Object.keys(expected as object);
  const ak = Object.keys(actual as object);
  if (!opts.extra && ek.length !== ak.length) {
    // keys with ${json-unit.ignore} may be absent
    const required = ek.filter((k) => !(typeof (expected as Record<string, unknown>)[k] === 'string' && /^\$\{json-unit\.ignore/.test((expected as Record<string, string>)[k])));
    if (ak.some((k) => !ek.includes(k))) return false;
    if (required.some((k) => !ak.includes(k))) return false;
  }
  return ek.every((k) => {
    const ev = (expected as Record<string, unknown>)[k];
    if (!(k in (actual as object))) return typeof ev === 'string' && /^\$\{json-unit\.ignore/.test(ev);
    return jsonEq(ev, (actual as Record<string, unknown>)[k], opts);
  });
}

// ---------------------------------------------------------------- XML

function xmlDoc(s: string): Document | undefined {
  if (typeof DOMParser === 'undefined') return undefined;
  const d = new DOMParser().parseFromString(s, 'application/xml');
  if (d.getElementsByTagName('parsererror').length) return undefined;
  return d;
}

function normalizeXml(n: Node): string {
  if (n.nodeType === 3) return (n.textContent ?? '').trim();
  if (n.nodeType !== 1 && n.nodeType !== 9) return '';
  const el = n as Element;
  const kids = [...n.childNodes].map(normalizeXml).filter(Boolean).join('');
  if (n.nodeType === 9) return kids;
  const attrs = [...el.attributes]
    .filter((a) => !a.name.startsWith('xmlns'))
    .map((a) => `${a.localName}="${a.value}"`)
    .sort()
    .join(' ');
  return `<${el.localName}${attrs ? ' ' + attrs : ''}>${kids}</${el.localName}>`;
}

// ---------------------------------------------------------------- string patterns

/** Match a single value (or undefined = absent) against a pattern. */
export function matchValue(p: ContentPattern, actual: string | undefined): { verdict: Verdict; note?: string } {
  const op = opOf(p);
  const v = (b: boolean): Verdict => (b ? 'match' : 'nomatch');
  if (op === 'absent') return { verdict: v(actual === undefined) };
  if (op === 'and') return { verdict: combine((p.and ?? []).map((x) => matchValue(x, actual).verdict)) };
  if (op === 'or') {
    const vs = (p.or ?? []).map((x) => matchValue(x, actual).verdict);
    return { verdict: vs.includes('match') ? 'match' : vs.includes('unknown') ? 'unknown' : 'nomatch' };
  }
  if (op === 'not') {
    const r = matchValue(p.not as ContentPattern, actual).verdict;
    return { verdict: r === 'unknown' ? 'unknown' : r === 'match' ? 'nomatch' : 'match' };
  }
  if (actual === undefined) return { verdict: 'nomatch', note: 'not present' };
  try {
    switch (op) {
      case 'equalTo':
        return { verdict: v(p.caseInsensitive ? actual.toLowerCase() === String(p.equalTo).toLowerCase() : actual === String(p.equalTo)) };
      case 'contains':
        return { verdict: v(p.caseInsensitive ? actual.toLowerCase().includes(String(p.contains).toLowerCase()) : actual.includes(String(p.contains))) };
      case 'doesNotContain':
        return { verdict: v(!actual.includes(String(p.doesNotContain))) };
      case 'matches':
        return { verdict: v(javaRegex(String(p.matches)).test(actual)) };
      case 'doesNotMatch':
        return { verdict: v(!javaRegex(String(p.doesNotMatch)).test(actual)) };
      case 'before':
      case 'after':
      case 'equalToDateTime': {
        const exp = parseExpectedDate(String(p[op]));
        const act = new Date(actual);
        if (!exp || Number.isNaN(act.getTime()) || p.actualFormat || p.truncateExpected || p.truncateActual) return { verdict: 'unknown', note: 'date format / truncation not evaluated' };
        if (op === 'before') return { verdict: v(act < exp) };
        if (op === 'after') return { verdict: v(act > exp) };
        return { verdict: v(act.getTime() === exp.getTime()) };
      }
      case 'equalToJson': {
        let exp = p.equalToJson;
        if (typeof exp === 'string') exp = JSON.parse(exp);
        let act: unknown;
        try {
          act = JSON.parse(actual);
        } catch {
          return { verdict: 'nomatch', note: 'body is not valid JSON' };
        }
        return { verdict: v(jsonEq(exp, act, { arr: !!p.ignoreArrayOrder, extra: !!p.ignoreExtraElements })) };
      }
      case 'matchesJsonPath': {
        let doc: unknown;
        try {
          doc = JSON.parse(actual);
        } catch {
          return { verdict: 'nomatch', note: 'body is not valid JSON' };
        }
        const mp = p.matchesJsonPath;
        const expr = typeof mp === 'string' ? mp : (mp as { expression: string }).expression;
        const res = evaluate(expr, doc);
        if (typeof mp === 'string' || !mp) {
          const hit = res.length > 0 && !(res.length === 1 && Array.isArray(res[0]) && res[0].length === 0 && !isDefinite(expr));
          return { verdict: v(hit), note: hit ? undefined : 'path not found' };
        }
        const { expression: _e, ...sub } = mp as { expression: string } & ContentPattern;
        void _e;
        if (!res.length) return matchValue(sub, undefined);
        const val = isDefinite(expr) ? res[0] : res;
        const asText = (x: unknown) => (typeof x === 'string' ? x : JSON.stringify(x));
        const whole = matchValue(sub, asText(val));
        // WireMock tries each element of an array result as well as the whole array.
        if (whole.verdict === 'match' || !Array.isArray(val) || !val.length) return whole;
        const each = val.map((x) => matchValue(sub, asText(x)).verdict);
        return each.includes('match') ? { verdict: 'match' } : each.includes('unknown') ? { verdict: 'unknown' } : whole;
      }
      case 'matchesXPath': {
        const doc = xmlDoc(actual);
        if (!doc) return { verdict: typeof DOMParser === 'undefined' ? 'unknown' : 'nomatch', note: 'body is not valid XML' };
        const mp = p.matchesXPath;
        const expr = typeof mp === 'string' ? mp : (mp as { expression: string }).expression;
        const ns = p.xPathNamespaces ?? {};
        const res = doc.evaluate(expr, doc, (prefix) => (prefix ? (ns[prefix] ?? null) : null), XPathResult.ANY_TYPE, null);
        const values: string[] = [];
        if (res.resultType === XPathResult.BOOLEAN_TYPE) values.push(String(res.booleanValue));
        else if (res.resultType === XPathResult.NUMBER_TYPE) values.push(String(res.numberValue));
        else if (res.resultType === XPathResult.STRING_TYPE) values.push(res.stringValue);
        else for (let n = res.iterateNext(); n; n = res.iterateNext()) values.push(n.textContent ?? '');
        if (typeof mp === 'string') return { verdict: v(values.length > 0 && values[0] !== 'false') };
        const { expression: _e, ...sub } = mp as { expression: string } & ContentPattern;
        void _e;
        const vs = values.map((x) => matchValue(sub, x).verdict);
        return { verdict: vs.includes('match') ? 'match' : vs.includes('unknown') ? 'unknown' : 'nomatch' };
      }
      case 'equalToXml': {
        if (p.enablePlaceholders) return { verdict: 'unknown', note: 'XML placeholders not evaluated' };
        const a = xmlDoc(actual);
        const e = xmlDoc(String(p.equalToXml));
        if (!a || !e) return { verdict: typeof DOMParser === 'undefined' ? 'unknown' : 'nomatch', note: 'invalid XML' };
        return { verdict: v(normalizeXml(a) === normalizeXml(e)) };
      }
      case 'equalToNumber':
      case 'greaterThanNumber':
      case 'greaterThanEqualNumber':
      case 'lessThanNumber':
      case 'lessThanEqualNumber': {
        // WireMock accepts the operand as a JSON number or a numeric string.
        const exp = Number(p[op]);
        if (Number.isNaN(exp)) return { verdict: 'unknown', note: `${op} operand is not a number` };
        const act = parseJavaDouble(actual);
        if (act === null) return { verdict: 'unknown', note: 'hexadecimal number not evaluated' };
        if (act === undefined) return { verdict: 'nomatch', note: 'not a number' };
        return { verdict: v(NUMBER_TEST[op](act, exp)) };
      }
      case 'binaryEqualTo':
        return { verdict: 'unknown', note: 'binary comparison done on server' };
      case 'matchesJsonSchema':
        return { verdict: 'unknown', note: 'JSON Schema validated on server' };
      case 'hasExactly':
      case 'includes':
        return matchMulti(p, [actual]);
    }
  } catch (e) {
    if (e instanceof JsonPathUnsupported || e instanceof SyntaxError) return { verdict: 'unknown', note: e.message };
    return { verdict: 'unknown', note: (e as Error).message };
  }
  return { verdict: 'unknown', note: `matcher ${op}` };
}

/** WireMock multi-value semantics: single patterns match if ANY value matches. */
function matchMulti(p: ContentPattern, values: string[] | undefined): { verdict: Verdict; note?: string } {
  const op = opOf(p);
  if (op === 'hasExactly') {
    const subs = p.hasExactly ?? [];
    if (!values || values.length !== subs.length) return { verdict: 'nomatch', note: `expected exactly ${subs.length} value(s)` };
    return { verdict: combine(subs.map((s) => (values.some((x) => matchValue(s, x).verdict === 'match') ? 'match' : 'nomatch'))) };
  }
  if (op === 'includes') {
    const subs = p.includes ?? [];
    return { verdict: combine(subs.map((s) => (values?.some((x) => matchValue(s, x).verdict === 'match') ? 'match' : 'nomatch'))) };
  }
  if (op === 'absent') return { verdict: !values || values.length === 0 ? 'match' : 'nomatch' };
  if (!values || values.length === 0) return matchValue(p, undefined);
  const rs = values.map((x) => matchValue(p, x));
  if (rs.some((r) => r.verdict === 'match')) return { verdict: 'match' };
  return rs.find((r) => r.verdict === 'unknown') ?? rs[0];
}

function describe(p: ContentPattern): string {
  const op = opOf(p);
  if (op === 'absent') return 'absent';
  if (isNumberOp(op)) return `${OP_LABEL[op]} ${p[op]}`;
  const raw = p[op];
  const val = typeof raw === 'string' ? raw : JSON.stringify(raw);
  return `${OP_LABEL[op] ?? op}${p.caseInsensitive ? ' (case-insensitive)' : ''}: ${val}`;
}

// ---------------------------------------------------------------- request

function headerValues(r: LoggedRequest, name: string): string[] | undefined {
  const k = Object.keys(r.headers ?? {}).find((h) => h.toLowerCase() === name.toLowerCase());
  if (!k) return undefined;
  const v = r.headers![k];
  return Array.isArray(v) ? v : [v];
}

function parseQuery(url: string): Map<string, string[]> {
  const m = new Map<string, string[]>();
  const q = url.split('?')[1]?.split('#')[0];
  if (!q) return m;
  for (const part of q.split('&')) {
    if (!part) continue;
    const [k, v = ''] = part.split('=');
    const key = safeDecode(k);
    m.set(key, [...(m.get(key) ?? []), safeDecode(v)]);
  }
  return m;
}

function safeDecode(s: string) {
  try {
    return decodeURIComponent(s.replace(/\+/g, ' '));
  } catch {
    return s;
  }
}

function parseCookies(r: LoggedRequest): Map<string, string[]> {
  const m = new Map<string, string[]>();
  for (const h of headerValues(r, 'cookie') ?? [])
    for (const part of h.split(';')) {
      const [k, ...v] = part.trim().split('=');
      if (k) m.set(k, [...(m.get(k) ?? []), v.join('=')]);
    }
  return m;
}

function matchTemplate(template: string, path: string): Record<string, string> | null {
  const names: string[] = [];
  const re = template
    .split(/(\{[^}]+\})/)
    .map((part) => {
      const m = /^\{([^}]+)\}$/.exec(part);
      if (m) {
        const name = m[1].replace(/^[*+.;/?&]/, '').replace(/\*$/, '');
        names.push(name);
        return m[1].endsWith('*') ? '(.+)' : '([^/]+)';
      }
      return part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    })
    .join('');
  const mm = new RegExp(`^${re}$`).exec(path);
  if (!mm) return null;
  return Object.fromEntries(names.map((n, i) => [n, safeDecode(mm[i + 1])]));
}

/**
 * WireMock's near-miss API does not report scenario state, so a scenario-gated stub can look
 * like a perfect match. Add the state as its own row: when every request field matched but the
 * request was unmatched, the scenario must have been in another state at the time.
 */
export function withScenario(report: MatchReport, stub: { scenarioName?: string; requiredScenarioState?: string }, currentState?: string, unmatched = true): MatchReport {
  if (!stub.scenarioName || !stub.requiredScenarioState) return report;
  const allMatched = report.verdict === 'match';
  const blamed = allMatched && unmatched;
  const field: FieldResult = {
    field: 'Scenario state',
    expected: `"${stub.scenarioName}" is ${stub.requiredScenarioState}`,
    actual: currentState ? `now ${currentState}` : '(not reported)',
    verdict: blamed ? 'nomatch' : currentState === stub.requiredScenarioState ? 'match' : 'unknown',
    note: blamed ? 'Every request field matched, so the scenario was in a different state when this request arrived.' : 'The state at request time is not recorded; showing the current state.',
  };
  return { verdict: blamed ? 'nomatch' : report.verdict, fields: [...report.fields, field] };
}

export function matchRequest(p: RequestPattern, r: LoggedRequest): MatchReport {
  const fields: FieldResult[] = [];
  const add = (field: string, expected: string, actual: string | undefined, verdict: Verdict, note?: string) => fields.push({ field, expected, actual: actual ?? '(absent)', verdict, note });

  const set = methodSetOf(p.method);
  const expectedMethod = set ? `${set.kind === 'oneOf' ? 'one of' : 'none of'} ${set.methods.join(', ')}` : methodLabel(p.method);
  add('Method', expectedMethod, r.method, methodAccepts(p.method, r.method) ? 'match' : 'nomatch');

  const url = r.url;
  const path = url.split('?')[0];
  const key = urlKeyOf(p);
  if (key) {
    const exp = String(p[key]);
    let verdict: Verdict = 'nomatch';
    let actual = path;
    try {
      if (key === 'url') {
        actual = url;
        verdict = url === exp ? 'match' : 'nomatch';
      } else if (key === 'urlPath') verdict = path === exp ? 'match' : 'nomatch';
      else if (key === 'urlPattern') {
        actual = url;
        verdict = javaRegex(exp).test(url) ? 'match' : 'nomatch';
      } else if (key === 'urlPathPattern') verdict = javaRegex(exp).test(path) ? 'match' : 'nomatch';
      else if (key === 'urlPathTemplate') {
        const vars = matchTemplate(exp, path);
        verdict = vars ? 'match' : 'nomatch';
        if (vars)
          for (const [name, pp] of Object.entries(p.pathParameters ?? {})) {
            const res = matchValue(pp, vars[name]);
            add(`Path param ${name}`, describe(pp), vars[name], res.verdict, res.note);
          }
      }
    } catch (e) {
      verdict = 'unknown';
      fields.push({ field: URL_KEY_LABEL[key], expected: exp, actual, verdict, note: (e as Error).message });
    }
    if (!fields.some((f) => f.field === URL_KEY_LABEL[key])) add(URL_KEY_LABEL[key], exp, actual, verdict);
  }

  const query = parseQuery(url);
  for (const [k, pp] of Object.entries(p.queryParameters ?? {})) {
    const vals = query.get(k);
    const res = matchMulti(pp, vals);
    add(`Query ${k}`, describe(pp), vals?.join(', '), res.verdict, res.note);
  }

  for (const [k, pp] of Object.entries(p.headers ?? {})) {
    const vals = headerValues(r, k);
    const res = matchMulti(pp, vals);
    add(k === DISABLED_HEADER ? 'Disabled (stub turned off in UI)' : `Header ${k}`, describe(pp), vals?.join(', '), res.verdict, res.note);
  }

  const cookies = parseCookies(r);
  for (const [k, pp] of Object.entries(p.cookies ?? {})) {
    const vals = cookies.get(k);
    const res = matchMulti(pp, vals);
    add(`Cookie ${k}`, describe(pp), vals?.join(', '), res.verdict, res.note);
  }

  if (p.formParameters) {
    const form = parseQuery(`?${r.body ?? ''}`);
    for (const [k, pp] of Object.entries(p.formParameters)) {
      const vals = form.get(k);
      const res = matchMulti(pp, vals);
      add(`Form ${k}`, describe(pp), vals?.join(', '), res.verdict, res.note);
    }
  }

  if (p.basicAuthCredentials) {
    const auth = headerValues(r, 'authorization')?.[0];
    let actual: string | undefined;
    if (auth?.startsWith('Basic ')) {
      try {
        actual = atob(auth.slice(6));
      } catch {
        actual = auth;
      }
    }
    const exp = `${p.basicAuthCredentials.username}:${p.basicAuthCredentials.password}`;
    add('Basic auth', exp, actual, actual === exp ? 'match' : 'nomatch');
  }

  if (p.host) {
    const host = r.host ?? headerValues(r, 'host')?.[0]?.split(':')[0];
    const res = matchValue(p.host, host);
    add('Host', describe(p.host), host, res.verdict, res.note);
  }
  if (p.port !== undefined) add('Port', String(p.port), r.port !== undefined ? String(r.port) : undefined, r.port === undefined ? 'unknown' : r.port === p.port ? 'match' : 'nomatch');
  if (p.scheme) add('Scheme', p.scheme, r.scheme, r.scheme ? (r.scheme === p.scheme ? 'match' : 'nomatch') : 'unknown');

  for (const [i, bp] of (p.bodyPatterns ?? []).entries()) {
    const body = r.body ?? (r.bodyAsBase64 ? safeAtob(r.bodyAsBase64) : undefined);
    const res = matchValue(bp, body || (opOf(bp) === 'absent' ? undefined : (body ?? '')));
    add(`Body${(p.bodyPatterns?.length ?? 0) > 1 ? ` #${i + 1}` : ''}`, describe(bp), truncate(body), res.verdict, res.note);
  }

  if (p.multipartPatterns?.length) add('Multipart', `${p.multipartPatterns.length} part pattern(s)`, '(see raw request)', 'unknown', 'multipart matching is evaluated on the server');
  if (p.customMatcher) add('Custom matcher', p.customMatcher.name, '—', 'unknown', 'extension matchers run on the server');

  return { verdict: combine(fields.map((f) => f.verdict)), fields };
}

function safeAtob(s: string) {
  try {
    return atob(s);
  } catch {
    return s;
  }
}

function truncate(s?: string, n = 2000) {
  if (s === undefined) return undefined;
  return s.length > n ? s.slice(0, n) + '…' : s;
}

// ---------------------------------------------------------------- server-side explanations

const SERVER_FIELD: Record<string, string> = {
  URL: 'URL equals',
  'URL path': 'Path equals',
  'URL pattern': 'URL regex',
  'URL path pattern': 'Path regex',
  'URL path template': 'Path template',
};

/** The engine's field results in this module's vocabulary (so `suggestFix` understands them). */
export function reportFromServer(fields: ServerFieldResult[]): MatchReport {
  const out: FieldResult[] = fields.map((f) => ({
    field: SERVER_FIELD[f.field] ?? f.field.replace(/^Path parameter /, 'Path param '),
    expected: f.expected,
    actual: f.actual ?? '(absent)',
    verdict: f.matched ? 'match' : 'nomatch',
    note: f.note,
  }));
  return { verdict: combine(out.map((f) => f.verdict)), fields: out };
}

/**
 * Explanation for a near miss: the server's own per-field evaluation when it provides one (the
 * built-in engine evaluates every matcher, JSON Schema and multipart included), otherwise the
 * client-side re-implementation plus the scenario check.
 */
export function explainNearMiss(
  stub: { request: RequestPattern; scenarioName?: string; requiredScenarioState?: string },
  req: LoggedRequest,
  result?: MatchResult,
  currentScenarioState?: string,
): MatchReport {
  if (result?.fields?.length) return reportFromServer(result.fields);
  return withScenario(matchRequest(stub.request, req), stub, currentScenarioState);
}
