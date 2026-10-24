// A small JSONPath evaluator (Goessner syntax subset) used by hosted mocks and monitors.
//
// Supported: `$`, `.name`, `['name']` / `["name"]`, `[n]` (negative from the end), `[a,b]`,
// `[start:end]`, `[*]` / `.*`, `..name` (recursive descent), and filters
// `[?(@.path)]`, `[?(@.path <op> literal)]` with ==, !=, <, <=, >, >=, =~ /regex/ and `&&` / `||`.
// Not supported: script expressions `[(…)]`, functions (`length()`), filter on the root (`$`).

export class JsonPathError extends Error {
  override name = "JsonPathError";
}

type Segment =
  | { t: "child"; names: (string | number)[] }
  | { t: "wild" }
  | { t: "slice"; start: number | null; end: number | null }
  | { t: "desc"; name: string | null }
  | { t: "filter"; expr: FilterExpr };

type FilterExpr =
  | { t: "exists"; path: Segment[] }
  | { t: "cmp"; path: Segment[]; op: string; value: unknown }
  | { t: "and" | "or"; a: FilterExpr; b: FilterExpr };

const cache = new Map<string, Segment[]>();

export function compileJsonPath(expr: string): Segment[] {
  const hit = cache.get(expr);
  if (hit) return hit;
  const segs = parsePath(expr.trim(), true);
  if (cache.size > 500) cache.clear();
  cache.set(expr, segs);
  return segs;
}

function parsePath(src: string, root: boolean): Segment[] {
  let i = 0;
  const prefix = root ? "$" : "@";
  if (src[0] === prefix) i = 1;
  else if (root && src[0] !== "." && src[0] !== "[") i = -1; // bare `a.b` → `$.a.b`
  const segs: Segment[] = [];
  if (i === -1) {
    i = 0;
    src = "." + src;
  }
  while (i < src.length) {
    const c = src[i];
    if (c === ".") {
      if (src[i + 1] === ".") {
        i += 2;
        if (src[i] === "*") {
          segs.push({ t: "desc", name: null });
          i++;
          continue;
        }
        if (src[i] === "[") {
          segs.push({ t: "desc", name: null });
          continue;
        }
        const m = /^[^.[\]\s]+/.exec(src.slice(i));
        if (!m) throw new JsonPathError(`expected a name after .. in ${src}`);
        segs.push({ t: "desc", name: m[0] });
        i += m[0].length;
        continue;
      }
      i++;
      if (src[i] === "*") {
        segs.push({ t: "wild" });
        i++;
        continue;
      }
      const m = /^[^.[\]\s()=!<>&|]+/.exec(src.slice(i));
      if (!m) throw new JsonPathError(`expected a name after . in ${src}`);
      segs.push({ t: "child", names: [m[0]] });
      i += m[0].length;
      continue;
    }
    if (c === "[") {
      const close = findClose(src, i);
      const inner = src.slice(i + 1, close).trim();
      i = close + 1;
      if (inner === "*") segs.push({ t: "wild" });
      else if (inner.startsWith("?")) {
        let body = inner.slice(1).trim();
        if (body.startsWith("(") && body.endsWith(")")) body = body.slice(1, -1);
        segs.push({ t: "filter", expr: parseFilter(body) });
      } else if (/^-?\d*\s*:\s*-?\d*$/.test(inner)) {
        const [a, b] = inner.split(":").map((x) => x.trim());
        segs.push({ t: "slice", start: a ? Number(a) : null, end: b ? Number(b) : null });
      } else {
        segs.push({ t: "child", names: splitUnion(inner).map(parseKey) });
      }
      continue;
    }
    if (/\s/.test(c!)) {
      i++;
      continue;
    }
    throw new JsonPathError(`unexpected "${c}" in ${src}`);
  }
  return segs;
}

function findClose(src: string, open: number): number {
  let depth = 0;
  let quote: string | null = null;
  for (let j = open; j < src.length; j++) {
    const ch = src[j]!;
    if (quote) {
      if (ch === "\\") j++;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"') quote = ch;
    else if (ch === "[") depth++;
    else if (ch === "]") {
      depth--;
      if (depth === 0) return j;
    }
  }
  throw new JsonPathError(`unclosed [ in ${src}`);
}

function splitUnion(s: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: string | null = null;
  for (let j = 0; j < s.length; j++) {
    const ch = s[j]!;
    if (quote) {
      cur += ch;
      if (ch === "\\") cur += s[++j] ?? "";
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      cur += ch;
    } else if (ch === ",") {
      out.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  out.push(cur.trim());
  return out.filter((x) => x.length);
}

function parseKey(k: string): string | number {
  if ((k.startsWith("'") && k.endsWith("'")) || (k.startsWith('"') && k.endsWith('"'))) return k.slice(1, -1).replace(/\\(.)/g, "$1");
  if (/^-?\d+$/.test(k)) return Number(k);
  return k;
}

function parseFilter(src: string): FilterExpr {
  const s = src.trim();
  const split = splitTop(s, "||");
  if (split) return { t: "or", a: parseFilter(split[0]), b: parseFilter(split[1]) };
  const and = splitTop(s, "&&");
  if (and) return { t: "and", a: parseFilter(and[0]), b: parseFilter(and[1]) };
  if (s.startsWith("(") && s.endsWith(")")) return parseFilter(s.slice(1, -1));
  const m = /^(@[^\s=!<>]*)\s*(==|!=|<=|>=|<|>|=~)\s*(.+)$/.exec(s);
  if (m) {
    const path = parsePath(m[1]!, false);
    const raw = m[3]!.trim();
    let value: unknown;
    if (m[2] === "=~") {
      const rm = /^\/(.*)\/([a-z]*)$/.exec(raw);
      if (!rm) throw new JsonPathError(`=~ needs /regex/ in ${src}`);
      value = new RegExp(rm[1]!, rm[2]!.replace(/[^imsu]/g, ""));
    } else value = parseLiteral(raw);
    return { t: "cmp", path, op: m[2]!, value };
  }
  if (s.startsWith("@")) return { t: "exists", path: parsePath(s, false) };
  throw new JsonPathError(`unsupported filter: ${src}`);
}

function splitTop(s: string, op: string): [string, string] | null {
  let depth = 0;
  let quote: string | null = null;
  for (let j = 0; j < s.length - 1; j++) {
    const ch = s[j]!;
    if (quote) {
      if (ch === "\\") j++;
      else if (ch === quote) quote = null;
      continue;
    }
    if (ch === "'" || ch === '"') quote = ch;
    else if (ch === "(" || ch === "[") depth++;
    else if (ch === ")" || ch === "]") depth--;
    else if (depth === 0 && s.startsWith(op, j)) return [s.slice(0, j), s.slice(j + op.length)];
  }
  return null;
}

function parseLiteral(raw: string): unknown {
  if ((raw.startsWith("'") && raw.endsWith("'")) || (raw.startsWith('"') && raw.endsWith('"'))) return raw.slice(1, -1);
  if (raw === "true") return true;
  if (raw === "false") return false;
  if (raw === "null") return null;
  const n = Number(raw);
  if (!Number.isNaN(n)) return n;
  throw new JsonPathError(`unsupported literal ${raw}`);
}

function children(v: unknown): unknown[] {
  if (Array.isArray(v)) return v;
  if (v && typeof v === "object") return Object.values(v as Record<string, unknown>);
  return [];
}

function descend(v: unknown, out: unknown[]): void {
  out.push(v);
  for (const c of children(v)) descend(c, out);
}

function step(nodes: unknown[], seg: Segment): unknown[] {
  const out: unknown[] = [];
  for (const n of nodes) {
    switch (seg.t) {
      case "child":
        for (const name of seg.names) {
          if (typeof name === "number") {
            if (Array.isArray(n)) {
              const idx = name < 0 ? n.length + name : name;
              if (idx >= 0 && idx < n.length) out.push(n[idx]);
            }
          } else if (n && typeof n === "object" && !Array.isArray(n) && Object.prototype.hasOwnProperty.call(n, name)) {
            out.push((n as Record<string, unknown>)[name]);
          } else if (Array.isArray(n) && name === "length") {
            out.push(n.length);
          }
        }
        break;
      case "wild":
        out.push(...children(n));
        break;
      case "slice":
        if (Array.isArray(n)) {
          const len = n.length;
          const norm = (x: number | null, d: number) => (x === null ? d : x < 0 ? Math.max(0, len + x) : Math.min(len, x));
          out.push(...n.slice(norm(seg.start, 0), norm(seg.end, len)));
        }
        break;
      case "desc": {
        const all: unknown[] = [];
        descend(n, all);
        if (seg.name === null) out.push(...all.flatMap((x) => (x === n ? [] : [x])));
        else for (const x of all) if (x && typeof x === "object" && !Array.isArray(x) && Object.prototype.hasOwnProperty.call(x, seg.name)) out.push((x as Record<string, unknown>)[seg.name]);
        break;
      }
      case "filter":
        // Like Jayway: arrays filter their elements, objects test themselves.
        if (Array.isArray(n)) {
          for (const c of n) if (testFilter(c, seg.expr)) out.push(c);
        } else if (n && typeof n === "object" && testFilter(n, seg.expr)) out.push(n);
        break;
    }
  }
  return out;
}

function evalSegs(root: unknown, segs: Segment[]): unknown[] {
  let nodes: unknown[] = [root];
  for (const seg of segs) nodes = step(nodes, seg);
  return nodes;
}

function testFilter(node: unknown, e: FilterExpr): boolean {
  switch (e.t) {
    case "and":
      return testFilter(node, e.a) && testFilter(node, e.b);
    case "or":
      return testFilter(node, e.a) || testFilter(node, e.b);
    case "exists":
      return evalSegs(node, e.path).length > 0;
    case "cmp": {
      const vals = evalSegs(node, e.path);
      if (!vals.length) return e.op === "!=";
      const v = vals[0];
      switch (e.op) {
        case "==":
          return v === e.value || (typeof v === "number" && typeof e.value === "string" && String(v) === e.value);
        case "!=":
          return v !== e.value;
        case "<":
          return typeof v === typeof e.value && (v as number) < (e.value as number);
        case "<=":
          return typeof v === typeof e.value && (v as number) <= (e.value as number);
        case ">":
          return typeof v === typeof e.value && (v as number) > (e.value as number);
        case ">=":
          return typeof v === typeof e.value && (v as number) >= (e.value as number);
        case "=~":
          return typeof v === "string" && (e.value as RegExp).test(v);
      }
      return false;
    }
  }
}

/** Every value selected by `expr` (empty when nothing matches). Throws JsonPathError on bad syntax. */
export function queryJsonPath(doc: unknown, expr: string): unknown[] {
  return evalSegs(doc, compileJsonPath(expr));
}

/** Whether the path is "definite" (selects at most one node: no wildcards, slices, filters or `..`). */
export function isDefinite(expr: string): boolean {
  return compileJsonPath(expr).every((s) => s.t === "child" && s.names.length === 1);
}

/** Single-value convenience: definite paths give the value, others the list (undefined = no match). */
export function readJsonPath(doc: unknown, expr: string): unknown {
  const vals = queryJsonPath(doc, expr);
  if (isDefinite(expr)) return vals[0];
  return vals.length ? vals : undefined;
}
