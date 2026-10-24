/**
 * Small JSONPath evaluator covering what stubs typically use:
 *   $.a.b   $['a']   $.a[0]   $.a[-1]   $.a[*]   $..name   $.a[0,2]   $.a[1:3]
 *   filters: [?(@.x == 'y')]  [?(@.n > 2)]  [?(@.flag)]  [?(@.x =~ /re/)]  && ||
 *   functions: .length() .size()
 * Throws on syntax it does not understand so callers can fall back to "unknown".
 */
export class JsonPathUnsupported extends Error {}

type Seg =
  | { t: 'prop'; name: string }
  | { t: 'idx'; idx: number[] }
  | { t: 'slice'; start?: number; end?: number }
  | { t: 'wild' }
  | { t: 'desc'; name?: string }
  | { t: 'filter'; expr: string }
  | { t: 'fn'; name: string };

function parse(path: string): Seg[] {
  let p = path.trim();
  if (!p.startsWith('$')) throw new JsonPathUnsupported('must start with $');
  p = p.slice(1);
  const segs: Seg[] = [];
  let i = 0;
  while (i < p.length) {
    const c = p[i];
    if (c === '.' && p[i + 1] === '.') {
      i += 2;
      const m = /^([A-Za-z_$][\w$-]*|\*)/.exec(p.slice(i));
      if (m) {
        segs.push({ t: 'desc', name: m[1] === '*' ? undefined : m[1] });
        i += m[1].length;
      } else segs.push({ t: 'desc' });
      continue;
    }
    if (c === '.') {
      i++;
      const m = /^([A-Za-z_$][\w$-]*|\*)(\(\))?/.exec(p.slice(i));
      if (!m) throw new JsonPathUnsupported(`bad segment at ${i}`);
      if (m[2]) segs.push({ t: 'fn', name: m[1] });
      else segs.push(m[1] === '*' ? { t: 'wild' } : { t: 'prop', name: m[1] });
      i += m[0].length;
      continue;
    }
    if (c === '[') {
      const end = findClose(p, i);
      const inner = p.slice(i + 1, end).trim();
      i = end + 1;
      if (inner === '*') segs.push({ t: 'wild' });
      else if (inner.startsWith('?')) segs.push({ t: 'filter', expr: inner.replace(/^\?\s*\(?/, '').replace(/\)\s*$/, '') });
      else if (/^['"]/.test(inner)) {
        const names = inner.split(/\s*,\s*/).map((s) => s.replace(/^['"]|['"]$/g, ''));
        if (names.length === 1) segs.push({ t: 'prop', name: names[0] });
        else throw new JsonPathUnsupported('multi-name');
      } else if (/^-?\d*:-?\d*$/.test(inner)) {
        const [a, b] = inner.split(':');
        segs.push({ t: 'slice', start: a ? Number(a) : undefined, end: b ? Number(b) : undefined });
      } else if (/^-?\d+(\s*,\s*-?\d+)*$/.test(inner)) segs.push({ t: 'idx', idx: inner.split(',').map((x) => Number(x.trim())) });
      else throw new JsonPathUnsupported(`unsupported [${inner}]`);
      continue;
    }
    throw new JsonPathUnsupported(`unexpected "${c}"`);
  }
  return segs;
}

function findClose(s: string, open: number): number {
  let depth = 0;
  let q: string | null = null;
  for (let i = open; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === q && s[i - 1] !== '\\') q = null;
      continue;
    }
    if (c === "'" || c === '"') q = c;
    else if (c === '[') depth++;
    else if (c === ']') {
      depth--;
      if (depth === 0) return i;
    }
  }
  throw new JsonPathUnsupported('unclosed [');
}

function children(v: unknown): unknown[] {
  if (Array.isArray(v)) return v;
  if (v && typeof v === 'object') return Object.values(v);
  return [];
}

function descendants(v: unknown, out: unknown[] = []): unknown[] {
  out.push(v);
  for (const c of children(v)) descendants(c, out);
  return out;
}

function evalOperand(tok: string, ctx: unknown, root: unknown): unknown {
  const t = tok.trim();
  if (t.startsWith('@') || t.startsWith('$')) {
    const r = evaluate((t.startsWith('@') ? '$' + t.slice(1) : t) || '$', t.startsWith('@') ? ctx : root);
    return r.length ? r[0] : undefined;
  }
  if (/^'.*'$|^".*"$/.test(t)) return t.slice(1, -1);
  if (t === 'true') return true;
  if (t === 'false') return false;
  if (t === 'null') return null;
  if (/^-?\d+(\.\d+)?$/.test(t)) return Number(t);
  if (/^\/.*\/[a-z]*$/.test(t)) return t;
  throw new JsonPathUnsupported(`operand ${t}`);
}

function evalFilter(expr: string, ctx: unknown, root: unknown): boolean {
  const ors = splitTop(expr, '||');
  if (ors.length > 1) return ors.some((e) => evalFilter(e, ctx, root));
  const ands = splitTop(expr, '&&');
  if (ands.length > 1) return ands.every((e) => evalFilter(e, ctx, root));
  let e = expr.trim();
  if (e.startsWith('(') && e.endsWith(')')) e = e.slice(1, -1);
  const m = /^(.*?)\s*(==|!=|<=|>=|<|>|=~|\bin\b|\bnin\b|\bcontains\b)\s*(.*)$/.exec(e);
  if (!m) {
    if (e.startsWith('!')) return evalOperand(e.slice(1), ctx, root) === undefined;
    return evalOperand(e, ctx, root) !== undefined;
  }
  const a = evalOperand(m[1], ctx, root);
  const op = m[2];
  if (op === '=~') {
    const rm = /^\/(.*)\/([a-z]*)$/.exec(m[3].trim());
    if (!rm) throw new JsonPathUnsupported('regex');
    return typeof a === 'string' && new RegExp(rm[1], rm[2]).test(a);
  }
  if (op === 'in' || op === 'nin') {
    const list = JSON.parse(m[3].replace(/'/g, '"')) as unknown[];
    const inc = list.some((x) => x === a);
    return op === 'in' ? inc : !inc;
  }
  const b = evalOperand(m[3], ctx, root);
  switch (op) {
    case '==':
      return a === b || (a != null && b != null && String(a) === String(b) && typeof a === typeof b);
    case '!=':
      return a !== b;
    case '<':
      return (a as number) < (b as number);
    case '>':
      return (a as number) > (b as number);
    case '<=':
      return (a as number) <= (b as number);
    case '>=':
      return (a as number) >= (b as number);
    case 'contains':
      return Array.isArray(a) ? a.includes(b) : typeof a === 'string' && a.includes(String(b));
  }
  return false;
}

function splitTop(s: string, sep: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let q: string | null = null;
  let cur = '';
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      cur += c;
      if (c === q) q = null;
      continue;
    }
    if (c === "'" || c === '"') q = c;
    if (c === '(' || c === '[') depth++;
    if (c === ')' || c === ']') depth--;
    if (depth === 0 && s.startsWith(sep, i)) {
      out.push(cur);
      cur = '';
      i += sep.length - 1;
      continue;
    }
    cur += c;
  }
  out.push(cur);
  return out;
}

/** Returns the list of matched values. */
export function evaluate(path: string, doc: unknown): unknown[] {
  const segs = parse(path);
  let cur: unknown[] = [doc];
  for (const s of segs) {
    const next: unknown[] = [];
    for (const v of cur) {
      switch (s.t) {
        case 'prop':
          if (v && typeof v === 'object' && !Array.isArray(v) && s.name in (v as object)) next.push((v as Record<string, unknown>)[s.name]);
          break;
        case 'idx':
          if (Array.isArray(v)) for (const i of s.idx) {
            const k = i < 0 ? v.length + i : i;
            if (k >= 0 && k < v.length) next.push(v[k]);
          }
          break;
        case 'slice':
          if (Array.isArray(v)) next.push(...v.slice(s.start, s.end));
          break;
        case 'wild':
          next.push(...children(v));
          break;
        case 'desc': {
          const all = descendants(v);
          if (s.name) {
            for (const d of all) if (d && typeof d === 'object' && !Array.isArray(d) && s.name in (d as object)) next.push((d as Record<string, unknown>)[s.name]);
          } else next.push(...all.slice(1));
          break;
        }
        case 'filter':
          for (const c of Array.isArray(v) ? v : [v]) if (evalFilter(s.expr, c, doc)) next.push(c);
          break;
        case 'fn':
          if (s.name === 'length' || s.name === 'size') next.push(Array.isArray(v) ? v.length : typeof v === 'string' ? v.length : v && typeof v === 'object' ? Object.keys(v).length : 0);
          else if (s.name === 'keys' && v && typeof v === 'object') next.push(Object.keys(v));
          else throw new JsonPathUnsupported(`function ${s.name}()`);
          break;
      }
    }
    cur = next;
  }
  return cur;
}

export function isDefinite(path: string): boolean {
  return !/\[\*\]|\.\*|\.\.|\[\?|\[-?\d+\s*,|:/.test(path);
}
