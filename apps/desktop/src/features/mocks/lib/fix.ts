import type { LoggedRequest, StubMapping, ContentPattern } from './types';
import { parseJavaDouble, type MatchReport } from './match';
import { isNumberOp, methodSetOf, opOf, urlKeyOf } from './stub-utils';

/**
 * Proposes the smallest edit to `stub` so that it would match `req`
 * ("Apply fix" from a near miss). Returns undefined if nothing safe to change.
 */
export function suggestFix(stub: StubMapping, req: LoggedRequest, report: MatchReport): { stub: StubMapping; summary: string } | undefined {
  const failing = report.fields.filter((f) => f.verdict === 'nomatch');
  if (!failing.length) return undefined;
  const s = structuredClone(stub);
  const r = s.request;
  const changes: string[] = [];
  const path = req.url.split('?')[0];
  const query = new URLSearchParams(req.url.split('?')[1] ?? '');
  const hdr = (name: string) => {
    const k = Object.keys(req.headers ?? {}).find((h) => h.toLowerCase() === name.toLowerCase());
    if (!k) return undefined;
    const v = req.headers![k];
    return Array.isArray(v) ? v[0] : v;
  };
  const retarget = (p: ContentPattern, actual: string | undefined): ContentPattern | undefined => {
    if (actual === undefined) return undefined; // drop requirement
    const op = opOf(p);
    if (op === 'absent') return undefined;
    if (isNumberOp(op)) {
      // Keep it numeric (WireMock 4): move the bound just far enough to include the actual value.
      const n = parseJavaDouble(actual);
      if (typeof n === 'number' && Number.isFinite(n)) {
        if (op === 'equalToNumber') return { equalToNumber: n };
        return op === 'greaterThanNumber' || op === 'greaterThanEqualNumber' ? { greaterThanEqualNumber: n } : { lessThanEqualNumber: n };
      }
    }
    return { equalTo: actual, ...(p.caseInsensitive ? { caseInsensitive: true } : {}) };
  };

  for (const f of failing) {
    if (f.field === 'Method') {
      // WireMock 4 method sets: widen the set rather than replacing it.
      const set = methodSetOf(r.method);
      if (set?.kind === 'oneOf') {
        r.method = { oneOf: [...set.methods, req.method] };
        changes.push(`method → one of ${[...set.methods, req.method].join(', ')}`);
      } else if (set?.kind === 'noneOf') {
        const rest = set.methods.filter((m) => m.toUpperCase() !== req.method.toUpperCase());
        r.method = rest.length ? { noneOf: rest } : 'ANY';
        changes.push(rest.length ? `method → none of ${rest.join(', ')}` : 'method → ANY');
      } else {
        r.method = req.method;
        changes.push(`method → ${req.method}`);
      }
    } else if (/^(URL equals|Path equals|URL regex|Path regex|Path template)$/.test(f.field)) {
      const k = urlKeyOf(r);
      if (k === 'url') r.url = req.url;
      else if (k === 'urlPath') r.urlPath = path;
      else {
        delete r[k!];
        r.urlPath = path;
      }
      changes.push(`URL → ${k === 'url' ? req.url : path}`);
    } else if (f.field.startsWith('Query ')) {
      const name = f.field.slice(6);
      const np = retarget(r.queryParameters![name], query.get(name) ?? undefined);
      if (np) r.queryParameters![name] = np;
      else delete r.queryParameters![name];
      changes.push(`query ${name}`);
    } else if (f.field.startsWith('Header ')) {
      const name = f.field.slice(7);
      const np = retarget(r.headers![name], hdr(name));
      if (np) r.headers![name] = np;
      else delete r.headers![name];
      changes.push(`header ${name}`);
    } else if (f.field.startsWith('Cookie ')) {
      const name = f.field.slice(7);
      delete r.cookies![name];
      changes.push(`cookie ${name} (removed)`);
    } else if (f.field.startsWith('Body')) {
      const idx = /#(\d+)/.exec(f.field) ? Number(/#(\d+)/.exec(f.field)![1]) - 1 : 0;
      const bp = r.bodyPatterns?.[idx];
      if (!bp) continue;
      const op = opOf(bp);
      if (op === 'equalToJson') {
        try {
          r.bodyPatterns![idx] = { ...bp, equalToJson: JSON.parse(req.body ?? '') };
          changes.push('JSON body → request body');
        } catch {
          /* not JSON */
        }
      } else if (op === 'equalTo' || op === 'equalToXml') {
        r.bodyPatterns![idx] = { ...bp, [op]: req.body ?? '' };
        changes.push('body → request body');
      } else {
        r.bodyPatterns!.splice(idx, 1);
        changes.push(`body ${op} (removed)`);
      }
    } else if (f.field.startsWith('Form ')) {
      const name = f.field.slice(5);
      delete r.formParameters![name];
      changes.push(`form ${name} (removed)`);
    } else if (f.field === 'Basic auth') {
      delete r.basicAuthCredentials;
      changes.push('basic auth (removed)');
    }
  }
  for (const k of ['queryParameters', 'headers', 'cookies', 'formParameters'] as const) if (r[k] && !Object.keys(r[k]!).length) delete r[k];
  if (r.bodyPatterns && !r.bodyPatterns.length) delete r.bodyPatterns;
  if (!changes.length) return undefined;
  return { stub: s, summary: `Changes: ${changes.join(', ')}` };
}
