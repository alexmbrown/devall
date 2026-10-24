// Response templating for hosted mocks: a well-defined subset of WireMock's Handlebars templating.
// Everything listed in relay/README.md ("Hosted mocks → supported WireMock subset") is implemented
// here; the desktop's publish check (crates/devall-relay/src/hosted_mock.rs) warns about the rest.
//
// Syntax: {{expr}} / {{{expr}}} (never HTML-escaped, like WireMock 3), {{! comments }}, `~`
// whitespace control, blocks {{#if}} {{#unless}} {{#each}} {{#with}} with {{else}}, and any
// condition helper as a block ({{#eq a b}}…{{else}}…{{/eq}}). Sub-expressions (helper …), string /
// number / boolean literals, hash arguments key=value, paths with .[n] / .[name] segments, `this`,
// `@index` `@key` `@first` `@last`, and `../` for the parent scope.

import { queryJsonPath, isDefinite, JsonPathError } from "./jsonpath";

export class TemplateError extends Error {
  override name = "TemplateError";
}

/** Limits that keep a hostile template from burning CPU or memory at the edge. */
export const MAX_OUTPUT = 2 * 1024 * 1024;
const MAX_ITERATIONS = 10_000;
const MAX_DEPTH = 32;

// ---------- values with WireMock semantics ----------

/** Query parameters / headers / cookies: renders its value (or `[a, b]`), indexable, iterable. */
export class Multi {
  constructor(readonly values: string[]) {}
  toString(): string {
    return this.values.length === 1 ? this.values[0]! : this.values.length === 0 ? "" : `[${this.values.join(", ")}]`;
  }
}

/** `request.path`: renders the path; `.[n]` = segment n, `.name` = urlPathTemplate variable. */
export class PathValue {
  constructor(
    readonly path: string,
    readonly segments: string[],
    readonly vars: Record<string, string>,
  ) {}
  toString(): string {
    return this.path;
  }
}

/** Case-insensitive bag (headers). */
export class Bag {
  private readonly map = new Map<string, Multi>();
  constructor(entries: [string, string][], readonly caseInsensitive: boolean) {
    for (const [k, v] of entries) {
      const key = caseInsensitive ? k.toLowerCase() : k;
      const cur = this.map.get(key);
      if (cur) cur.values.push(v);
      else this.map.set(key, new Multi([v]));
    }
  }
  get(name: string): Multi | undefined {
    return this.map.get(this.caseInsensitive ? name.toLowerCase() : name);
  }
  keys(): string[] {
    return [...this.map.keys()];
  }
}

// ---------- AST ----------

type Expr =
  | { t: "path"; parts: string[]; up: number; data: boolean }
  | { t: "lit"; v: unknown }
  | { t: "call"; name: string; args: Expr[]; hash: Record<string, Expr> };

type Node =
  | { t: "text"; v: string }
  | { t: "out"; e: Expr }
  | { t: "block"; name: string; args: Expr[]; hash: Record<string, Expr>; body: Node[]; inverse: Node[] };

interface Tok {
  kind: "text" | "mustache";
  v: string;
  stripBefore?: boolean;
  stripAfter?: boolean;
}

function tokenize(src: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  while (i < src.length) {
    const open = src.indexOf("{{", i);
    if (open < 0) {
      out.push({ kind: "text", v: src.slice(i) });
      break;
    }
    if (open > i) out.push({ kind: "text", v: src.slice(i, open) });
    // Escaped mustache: \{{ renders literally.
    if (open > 0 && src[open - 1] === "\\") {
      const last = out[out.length - 1];
      if (last?.kind === "text") last.v = last.v.slice(0, -1);
      out.push({ kind: "text", v: "{{" });
      i = open + 2;
      continue;
    }
    const triple = src.startsWith("{{{", open);
    let close: number;
    let inner: string;
    if (src.startsWith("{{!--", open)) {
      close = src.indexOf("--}}", open);
      if (close < 0) throw new TemplateError("unclosed {{!-- comment");
      i = close + 4;
      continue;
    }
    if (triple) {
      close = src.indexOf("}}}", open + 3);
      if (close < 0) throw new TemplateError("unclosed {{{");
      inner = src.slice(open + 3, close);
      i = close + 3;
    } else {
      close = src.indexOf("}}", open + 2);
      if (close < 0) throw new TemplateError("unclosed {{");
      inner = src.slice(open + 2, close);
      i = close + 2;
    }
    let stripBefore = false;
    let stripAfter = false;
    if (inner.startsWith("~")) {
      stripBefore = true;
      inner = inner.slice(1);
    }
    if (inner.endsWith("~")) {
      stripAfter = true;
      inner = inner.slice(0, -1);
    }
    if (inner.trimStart().startsWith("!")) {
      out.push({ kind: "text", v: "", stripBefore, stripAfter });
      continue;
    }
    out.push({ kind: "mustache", v: inner.trim(), stripBefore, stripAfter });
  }
  // Apply whitespace control.
  for (let k = 0; k < out.length; k++) {
    const t = out[k]!;
    if (t.stripBefore && k > 0 && out[k - 1]!.kind === "text") out[k - 1]!.v = out[k - 1]!.v.replace(/\s+$/, "");
    if (t.stripAfter && k + 1 < out.length && out[k + 1]!.kind === "text") out[k + 1]!.v = out[k + 1]!.v.replace(/^\s+/, "");
  }
  return out;
}

// Expression lexer: words, strings, parens, `=`.
function lexExpr(s: string): string[] {
  const toks: string[] = [];
  let i = 0;
  while (i < s.length) {
    const c = s[i]!;
    if (/\s/.test(c)) {
      i++;
      continue;
    }
    if (c === "(" || c === ")") {
      toks.push(c);
      i++;
      continue;
    }
    if (c === "'" || c === '"') {
      let j = i + 1;
      let v = "";
      while (j < s.length && s[j] !== c) {
        if (s[j] === "\\" && j + 1 < s.length && (s[j + 1] === c || s[j + 1] === "\\")) {
          v += s[j + 1];
          j += 2;
        } else v += s[j++];
      }
      if (j >= s.length) throw new TemplateError(`unclosed string in {{${s}}}`);
      toks.push(JSON.stringify(v)); // marker: JSON string literal
      i = j + 1;
      continue;
    }
    if (c === "=") {
      toks.push("=");
      i++;
      continue;
    }
    let j = i;
    let depth = 0;
    while (j < s.length) {
      const ch = s[j]!;
      if (ch === "[") depth++;
      else if (ch === "]") depth--;
      else if (depth === 0 && (/\s/.test(ch) || ch === "(" || ch === ")" || ch === "=")) break;
      j++;
    }
    toks.push(s.slice(i, j));
    i = j;
  }
  return toks;
}

function parsePathExpr(word: string): Expr {
  let w = word;
  let up = 0;
  while (w.startsWith("../")) {
    up++;
    w = w.slice(3);
  }
  const data = w.startsWith("@");
  if (data) w = w.slice(1);
  if (w === "this" || w === ".") return { t: "path", parts: [], up, data };
  if (w.startsWith("this.")) w = w.slice(5);
  const parts: string[] = [];
  let cur = "";
  for (let i = 0; i < w.length; i++) {
    const ch = w[i]!;
    if (ch === "[") {
      const close = w.indexOf("]", i);
      if (close < 0) throw new TemplateError(`unclosed [ in ${word}`);
      if (cur) parts.push(cur);
      cur = "";
      parts.push(w.slice(i + 1, close));
      i = close;
      if (w[i + 1] === ".") i++;
      continue;
    }
    if (ch === "." || ch === "/") {
      if (cur) parts.push(cur);
      cur = "";
      continue;
    }
    cur += ch;
  }
  if (cur) parts.push(cur);
  return { t: "path", parts, up, data };
}

function atom(word: string): Expr {
  if (word.startsWith('"')) return { t: "lit", v: JSON.parse(word) as string };
  if (word === "true") return { t: "lit", v: true };
  if (word === "false") return { t: "lit", v: false };
  if (word === "null" || word === "undefined") return { t: "lit", v: null };
  if (/^-?\d+(\.\d+)?$/.test(word)) return { t: "lit", v: Number(word) };
  return parsePathExpr(word);
}

/** Parses `name arg arg key=value` from tokens [pos, end). */
function parseCall(toks: string[], pos: { i: number }, stopAtParen: boolean): Expr {
  const args: Expr[] = [];
  const hash: Record<string, Expr> = {};
  let name: string | null = null;
  while (pos.i < toks.length) {
    const t = toks[pos.i]!;
    if (t === ")") {
      if (!stopAtParen) throw new TemplateError("unexpected )");
      break;
    }
    let e: Expr;
    if (t === "(") {
      pos.i++;
      e = parseCall(toks, pos, true);
      if (toks[pos.i] !== ")") throw new TemplateError("missing )");
      pos.i++;
    } else if (toks[pos.i + 1] === "=" && /^[A-Za-z_][\w-]*$/.test(t)) {
      pos.i += 2;
      const vt = toks[pos.i];
      if (vt === undefined) throw new TemplateError(`missing value for ${t}=`);
      if (vt === "(") {
        pos.i++;
        hash[t] = parseCall(toks, pos, true);
        if (toks[pos.i] !== ")") throw new TemplateError("missing )");
        pos.i++;
      } else {
        hash[t] = atom(vt);
        pos.i++;
      }
      continue;
    } else {
      e = atom(t);
      pos.i++;
    }
    if (name === null && args.length === 0 && Object.keys(hash).length === 0 && e.t === "path" && !e.data && e.up === 0 && e.parts.length === 1 && !t.startsWith("(")) {
      name = e.parts[0]!;
      // Keep it as a potential plain path too (resolved at render time when no helper exists).
      args.push(e);
      continue;
    }
    args.push(e);
  }
  if (name !== null) {
    const rest = args.slice(1);
    return { t: "call", name, args: rest, hash };
  }
  if (args.length === 1 && Object.keys(hash).length === 0) return args[0]!;
  throw new TemplateError("expected a helper name");
}

function parseExpr(src: string): Expr {
  const toks = lexExpr(src);
  if (!toks.length) throw new TemplateError("empty {{}}");
  const pos = { i: 0 };
  const e = parseCall(toks, pos, false);
  if (pos.i < toks.length) throw new TemplateError(`unexpected ${toks[pos.i]} in {{${src}}}`);
  return e;
}

function parse(src: string): Node[] {
  const toks = tokenize(src);
  let k = 0;
  const parseUntil = (closer: string | null): { nodes: Node[]; inverse: Node[] } => {
    const nodes: Node[] = [];
    let inverse: Node[] | null = null;
    const target = () => inverse ?? nodes;
    while (k < toks.length) {
      const t = toks[k++]!;
      if (t.kind === "text") {
        if (t.v) target().push({ t: "text", v: t.v });
        continue;
      }
      const v = t.v;
      if (v.startsWith("/")) {
        const name = v.slice(1).trim();
        if (closer === null || name !== closer) throw new TemplateError(`unexpected {{/${name}}}`);
        return { nodes, inverse: inverse ?? [] };
      }
      if (v === "else" || v === "^") {
        if (closer === null) throw new TemplateError("{{else}} outside a block");
        inverse = [];
        continue;
      }
      if (v.startsWith("else ")) {
        // {{else if x}} → nested if in the inverse
        if (closer === null) throw new TemplateError("{{else}} outside a block");
        const inner = v.slice(5).trim();
        inverse = [];
        const call = parseExpr(inner);
        if (call.t !== "call") throw new TemplateError(`{{else ${inner}}} needs a helper`);
        const sub = parseUntil(closer);
        inverse.push({ t: "block", name: call.name, args: call.args, hash: call.hash, body: sub.nodes, inverse: sub.inverse });
        return { nodes, inverse };
      }
      if (v.startsWith("#")) {
        const e = parseExpr(v.slice(1));
        if (e.t !== "call") throw new TemplateError(`{{#${v.slice(1)}}} needs a helper name`);
        const sub = parseUntil(e.name);
        target().push({ t: "block", name: e.name, args: e.args, hash: e.hash, body: sub.nodes, inverse: sub.inverse });
        continue;
      }
      target().push({ t: "out", e: parseExpr(v) });
    }
    if (closer !== null) throw new TemplateError(`missing {{/${closer}}}`);
    return { nodes, inverse: inverse ?? [] };
  };
  return parseUntil(null).nodes;
}

const parsed = new Map<string, Node[]>();

function compile(src: string): Node[] {
  const hit = parsed.get(src);
  if (hit) return hit;
  const nodes = parse(src);
  if (parsed.size > 300) parsed.clear();
  parsed.set(src, nodes);
  return nodes;
}

// ---------- rendering ----------

interface Frame {
  ctx: unknown;
  data: Record<string, unknown>;
}

export interface RenderState {
  out: { n: number; iterations: number };
  now: () => number;
}

export function renderValue(v: unknown): string {
  if (v === null || v === undefined) return "";
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  if (v instanceof Multi || v instanceof PathValue) return v.toString();
  if (v instanceof Bag) return JSON.stringify(Object.fromEntries(v.keys().map((k) => [k, v.get(k)!.toString()])));
  if (v instanceof Date) return v.toISOString();
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

function truthy(v: unknown): boolean {
  if (v instanceof Multi) return v.values.length > 0 && v.values.some((x) => x !== "");
  if (Array.isArray(v)) return v.length > 0;
  if (typeof v === "string") return v.length > 0 && v !== "false";
  return !!v && v !== 0;
}

function prop(v: unknown, key: string): unknown {
  if (v === null || v === undefined) return undefined;
  if (v instanceof Multi) {
    if (/^\d+$/.test(key)) return v.values[Number(key)];
    if (key === "length" || key === "size") return v.values.length;
    if (key === "first") return v.values[0];
    if (key === "last") return v.values[v.values.length - 1];
    return undefined;
  }
  if (v instanceof PathValue) {
    if (/^\d+$/.test(key)) return v.segments[Number(key)];
    return v.vars[key];
  }
  if (v instanceof Bag) return v.get(key);
  if (Array.isArray(v)) {
    if (/^-?\d+$/.test(key)) {
      const n = Number(key);
      return v[n < 0 ? v.length + n : n];
    }
    if (key === "length" || key === "size") return v.length;
    return undefined;
  }
  if (typeof v === "string") {
    if (key === "length") return v.length;
    return undefined;
  }
  if (typeof v === "object") {
    const o = v as Record<string, unknown>;
    return Object.prototype.hasOwnProperty.call(o, key) ? o[key] : undefined;
  }
  return undefined;
}

function lookup(stack: Frame[], e: Extract<Expr, { t: "path" }>): unknown {
  const frame = stack[Math.max(0, stack.length - 1 - e.up)]!;
  if (e.data) {
    let v: unknown = frame.data[e.parts[0] ?? ""];
    for (const p of e.parts.slice(1)) v = prop(v, p);
    return v;
  }
  if (!e.parts.length) return frame.ctx;
  let v = prop(frame.ctx, e.parts[0]!);
  // Fall back to the root scope for top-level names (request, parameters…) inside blocks.
  if (v === undefined && e.up === 0) for (let i = stack.length - 2; i >= 0 && v === undefined; i--) v = prop(stack[i]!.ctx, e.parts[0]!);
  for (const p of e.parts.slice(1)) v = prop(v, p);
  return v;
}

function evaluate(e: Expr, stack: Frame[], st: RenderState, depth: number): unknown {
  if (depth > MAX_DEPTH) throw new TemplateError("template nests too deeply");
  switch (e.t) {
    case "lit":
      return e.v;
    case "path":
      return lookup(stack, e);
    case "call": {
      const helper = HELPERS[e.name];
      if (!helper) {
        if (e.args.length === 0 && Object.keys(e.hash).length === 0) return lookup(stack, { t: "path", parts: [e.name], up: 0, data: false });
        throw new TemplateError(`unsupported helper "${e.name}"`);
      }
      const args = e.args.map((a) => evaluate(a, stack, st, depth + 1));
      const hash: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(e.hash)) hash[k] = evaluate(v, stack, st, depth + 1);
      return helper(args, hash, st);
    }
  }
}

function emit(parts: string[], s: string, st: RenderState): void {
  st.out.n += s.length;
  if (st.out.n > MAX_OUTPUT) throw new TemplateError(`template output exceeds ${MAX_OUTPUT} bytes`);
  parts.push(s);
}

function iterate(v: unknown): [unknown, string | number][] {
  if (v instanceof Multi) return v.values.map((x, i) => [x, i]);
  if (v instanceof Bag) return v.keys().map((k) => [v.get(k)!, k]);
  if (Array.isArray(v)) return v.map((x, i) => [x, i]);
  if (v && typeof v === "object") return Object.entries(v as Record<string, unknown>).map(([k, x]) => [x, k]);
  return [];
}

function renderNodes(nodes: Node[], stack: Frame[], st: RenderState, parts: string[], depth: number): void {
  if (depth > MAX_DEPTH) throw new TemplateError("template nests too deeply");
  for (const n of nodes) {
    if (n.t === "text") {
      emit(parts, n.v, st);
      continue;
    }
    if (n.t === "out") {
      emit(parts, renderValue(evaluate(n.e, stack, st, depth)), st);
      continue;
    }
    const top = stack[stack.length - 1]!;
    const args = n.args.map((a) => evaluate(a, stack, st, depth));
    switch (n.name) {
      case "if":
      case "unless": {
        const cond = truthy(args[0]);
        renderNodes((n.name === "if" ? cond : !cond) ? n.body : n.inverse, stack, st, parts, depth + 1);
        break;
      }
      case "with": {
        if (truthy(args[0])) renderNodes(n.body, [...stack, { ctx: args[0], data: top.data }], st, parts, depth + 1);
        else renderNodes(n.inverse, stack, st, parts, depth + 1);
        break;
      }
      case "each": {
        const items = iterate(args[0]);
        if (!items.length) {
          renderNodes(n.inverse, stack, st, parts, depth + 1);
          break;
        }
        items.forEach(([item, key], i) => {
          st.out.iterations++;
          if (st.out.iterations > MAX_ITERATIONS) throw new TemplateError(`#each ran more than ${MAX_ITERATIONS} times`);
          const data = { ...top.data, index: i, key, first: i === 0, last: i === items.length - 1 };
          renderNodes(n.body, [...stack, { ctx: item, data }], st, parts, depth + 1);
        });
        break;
      }
      default: {
        const helper = HELPERS[n.name];
        if (!helper) throw new TemplateError(`unsupported block helper "#${n.name}"`);
        const hash: Record<string, unknown> = {};
        for (const [k, v] of Object.entries(n.hash)) hash[k] = evaluate(v, stack, st, depth);
        const v = helper(args, hash, st);
        renderNodes(truthy(v) ? n.body : n.inverse, stack, st, parts, depth + 1);
      }
    }
  }
}

/** Renders `template` against `model` (`{ request, parameters }`). Throws TemplateError. */
export function renderTemplate(template: string, model: Record<string, unknown>, now: () => number = Date.now): string {
  if (!template.includes("{{")) return template;
  const nodes = compile(template);
  const parts: string[] = [];
  renderNodes(nodes, [{ ctx: model, data: {} }], { out: { n: 0, iterations: 0 }, now }, parts, 0);
  return parts.join("");
}

/** Helper names this engine implements (the desktop's publish check mirrors this list). */
export function supportedHelpers(): string[] {
  return [...Object.keys(HELPERS), "if", "unless", "each", "with"].sort();
}

// ---------- helpers ----------

type Helper = (args: unknown[], hash: Record<string, unknown>, st: RenderState) => unknown;

function str(v: unknown): string {
  return renderValue(v);
}

function num(v: unknown): number {
  if (typeof v === "number") return v;
  const n = Number(str(v).trim());
  return Number.isNaN(n) ? 0 : n;
}

function list(v: unknown): unknown[] {
  if (v instanceof Multi) return v.values;
  if (Array.isArray(v)) return v;
  if (v === undefined || v === null) return [];
  return [v];
}

function cmp(a: unknown, b: unknown): number {
  const na = Number(str(a));
  const nb = Number(str(b));
  if (str(a).trim() !== "" && str(b).trim() !== "" && !Number.isNaN(na) && !Number.isNaN(nb)) return na - nb;
  return str(a) < str(b) ? -1 : str(a) > str(b) ? 1 : 0;
}

function randomInt(lo: number, hi: number): number {
  // inclusive bounds, like WireMock's randomInt (upper exclusive in WireMock 3 docs is ambiguous; we keep upper inclusive)
  const span = Math.max(0, Math.floor(hi) - Math.ceil(lo));
  const buf = new Uint32Array(1);
  crypto.getRandomValues(buf);
  return Math.ceil(lo) + (buf[0]! % (span + 1));
}

const RANDOM_SETS: Record<string, string> = {
  ALPHANUMERIC: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
  ALPHABETIC: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
  NUMERIC: "0123456789",
  HEXADECIMAL: "0123456789abcdef",
  ALPHANUMERIC_AND_SYMBOLS: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*()-_=+[]{};:,.<>/?",
};

function randomString(length: number, set: string): string {
  let s = "";
  for (let i = 0; i < length; i++) s += set[randomInt(0, set.length - 1)];
  return s;
}

// ---- dates ----

const UNIT_MS: Record<string, number> = { second: 1000, minute: 60_000, hour: 3_600_000, day: 86_400_000, week: 604_800_000 };

/** `3 days`, `-1 hours`, `2 months`, `1 years`, `30 seconds`. */
export function applyOffset(ms: number, offset: string): number {
  const m = /^\s*(-?\d+)\s+([a-z]+?)s?\s*$/i.exec(offset);
  if (!m) throw new TemplateError(`bad offset "${offset}" (use e.g. "3 days" or "-1 hours")`);
  const n = Number(m[1]);
  const unit = m[2]!.toLowerCase();
  if (unit === "month" || unit === "year") {
    const d = new Date(ms);
    if (unit === "month") d.setUTCMonth(d.getUTCMonth() + n);
    else d.setUTCFullYear(d.getUTCFullYear() + n);
    return d.getTime();
  }
  const u = UNIT_MS[unit];
  if (!u) throw new TemplateError(`bad offset unit "${m[2]}"`);
  return ms + n * u;
}

const pad = (n: number, w = 2) => String(n).padStart(w, "0");
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/** Java SimpleDateFormat subset in UTC: yyyy yy MM MMM dd d HH H hh mm ss SSS a EEE Z X 'literal', plus `epoch` and `unix`. */
export function formatDate(ms: number, format: string | undefined): string {
  if (!format) return new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");
  if (format === "epoch") return String(ms);
  if (format === "unix") return String(Math.floor(ms / 1000));
  const d = new Date(ms);
  let out = "";
  for (let i = 0; i < format.length; ) {
    const ch = format[i]!;
    if (ch === "'") {
      const end = format.indexOf("'", i + 1);
      if (end === i + 1) {
        out += "'";
        i += 2;
        continue;
      }
      out += format.slice(i + 1, end < 0 ? format.length : end);
      i = end < 0 ? format.length : end + 1;
      continue;
    }
    let j = i;
    while (j < format.length && format[j] === ch) j++;
    const run = j - i;
    i = j;
    switch (ch) {
      case "y":
        out += run === 2 ? pad(d.getUTCFullYear() % 100) : String(d.getUTCFullYear());
        break;
      case "M":
        out += run >= 3 ? MONTHS[d.getUTCMonth()]! : run === 2 ? pad(d.getUTCMonth() + 1) : String(d.getUTCMonth() + 1);
        break;
      case "d":
        out += run === 2 ? pad(d.getUTCDate()) : String(d.getUTCDate());
        break;
      case "H":
        out += run === 2 ? pad(d.getUTCHours()) : String(d.getUTCHours());
        break;
      case "h": {
        const h = d.getUTCHours() % 12 || 12;
        out += run === 2 ? pad(h) : String(h);
        break;
      }
      case "m":
        out += run === 2 ? pad(d.getUTCMinutes()) : String(d.getUTCMinutes());
        break;
      case "s":
        out += run === 2 ? pad(d.getUTCSeconds()) : String(d.getUTCSeconds());
        break;
      case "S":
        out += pad(d.getUTCMilliseconds(), 3).slice(0, run);
        break;
      case "a":
        out += d.getUTCHours() < 12 ? "AM" : "PM";
        break;
      case "E":
        out += DAYS[d.getUTCDay()]!;
        break;
      case "Z":
        out += "+0000";
        break;
      case "X":
        out += "Z";
        break;
      default:
        out += ch.repeat(run);
    }
  }
  return out;
}

// ---- faker-lite ----

const FIRST = ["Ada", "Alan", "Grace", "Linus", "Margaret", "Dennis", "Barbara", "Ken", "Radia", "Tim", "Katherine", "Guido", "Frances", "Bjarne", "Hedy", "John", "Mary", "Sophie", "Liam", "Noah", "Emma", "Olivia", "Lucas", "Mia"];
const LAST = ["Lovelace", "Turing", "Hopper", "Torvalds", "Hamilton", "Ritchie", "Liskov", "Thompson", "Perlman", "Berners-Lee", "Johnson", "van Rossum", "Allen", "Stroustrup", "Lamarr", "Smith", "Garcia", "Müller", "Rossi", "Kim", "Nguyen", "Silva"];
const CITIES = ["Lisbon", "Berlin", "Austin", "Toronto", "Osaka", "Nairobi", "Melbourne", "Oslo", "Bogotá", "Seoul", "Dublin", "Lyon"];
const COUNTRIES = ["Portugal", "Germany", "United States", "Canada", "Japan", "Kenya", "Australia", "Norway", "Colombia", "South Korea", "Ireland", "France"];
const STREETS = ["Main St", "Oak Ave", "Maple Rd", "Elm St", "Park Ln", "Cedar Ct", "Pine Dr", "Lake View"];
const COMPANIES = ["Acme", "Globex", "Initech", "Umbrella", "Stark Industries", "Wayne Enterprises", "Hooli", "Vandelay"];
const WORDS = ["lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do", "eiusmod", "tempor", "incididunt", "ut", "labore", "et", "dolore", "magna", "aliqua"];
const COLORS = ["red", "green", "blue", "orange", "purple", "teal", "yellow", "black", "white", "gray"];

function pick<T>(a: readonly T[]): T {
  return a[randomInt(0, a.length - 1)]!;
}

function uuid(): string {
  return crypto.randomUUID();
}

/** `{{random 'Name.firstName'}}` keys (case-insensitive; `first_name` = `firstName`). */
export const FAKER: Record<string, () => string> = {
  "name.firstname": () => pick(FIRST),
  "name.lastname": () => pick(LAST),
  "name.fullname": () => `${pick(FIRST)} ${pick(LAST)}`,
  "name.name": () => `${pick(FIRST)} ${pick(LAST)}`,
  "name.username": () => `${pick(FIRST).toLowerCase()}${randomInt(1, 999)}`,
  "internet.emailaddress": () => `${pick(FIRST).toLowerCase()}.${pick(LAST).toLowerCase().replace(/[^a-z]/g, "")}@example.com`,
  "internet.safeemailaddress": () => `${pick(FIRST).toLowerCase()}@example.com`,
  "internet.url": () => `https://www.${pick(COMPANIES).toLowerCase().replace(/[^a-z]/g, "")}.example`,
  "internet.domainname": () => `${pick(COMPANIES).toLowerCase().replace(/[^a-z]/g, "")}.example`,
  "internet.uuid": uuid,
  "internet.ipv4address": () => `${randomInt(1, 223)}.${randomInt(0, 255)}.${randomInt(0, 255)}.${randomInt(1, 254)}`,
  "internet.password": () => randomString(12, RANDOM_SETS.ALPHANUMERIC_AND_SYMBOLS!),
  "address.city": () => pick(CITIES),
  "address.country": () => pick(COUNTRIES),
  "address.streetaddress": () => `${randomInt(1, 999)} ${pick(STREETS)}`,
  "address.streetname": () => pick(STREETS),
  "address.zipcode": () => pad(randomInt(1000, 99999), 5),
  "address.latitude": () => (Math.random() * 180 - 90).toFixed(6),
  "address.longitude": () => (Math.random() * 360 - 180).toFixed(6),
  "company.name": () => pick(COMPANIES),
  "phonenumber.phonenumber": () => `+1-555-${pad(randomInt(0, 999), 3)}-${pad(randomInt(0, 9999), 4)}`,
  "phonenumber.cellphone": () => `+1-555-${pad(randomInt(0, 999), 3)}-${pad(randomInt(0, 9999), 4)}`,
  "lorem.word": () => pick(WORDS),
  "lorem.sentence": () => {
    const n = randomInt(5, 10);
    const ws = Array.from({ length: n }, () => pick(WORDS));
    return ws.join(" ").replace(/^./, (c) => c.toUpperCase()) + ".";
  },
  "lorem.paragraph": () => Array.from({ length: randomInt(3, 5) }, () => FAKER["lorem.sentence"]!()).join(" "),
  "number.digit": () => String(randomInt(0, 9)),
  "number.randomnumber": () => String(randomInt(0, 999_999)),
  "color.name": () => pick(COLORS),
  "bool.bool": () => String(randomInt(0, 1) === 1),
  "idnumber.valid": () => pad(randomInt(0, 999_999_999), 9),
};

function fakerKey(k: string): string {
  const [a = "", b = ""] = k.split(".");
  const norm = (s: string) => s.replace(/_/g, "").toLowerCase();
  return `${norm(a)}.${norm(b)}`;
}

// ---- JSON ----

function asJson(v: unknown): unknown {
  if (typeof v === "string") {
    try {
      return JSON.parse(v);
    } catch {
      return v;
    }
  }
  if (v instanceof Multi) return asJson(v.toString());
  return v;
}

const HELPERS: Record<string, Helper> = {
  // strings
  upper: ([v]) => str(v).toUpperCase(),
  lower: ([v]) => str(v).toLowerCase(),
  capitalize: ([v]) => str(v).replace(/\b\p{L}/gu, (c) => c.toUpperCase()),
  capitalizeFirst: ([v]) => str(v).replace(/^./, (c) => c.toUpperCase()),
  trim: ([v]) => str(v).trim(),
  replace: ([v, a, b]) => str(v).split(str(a)).join(str(b)),
  substring: ([v, s, e]) => str(v).substring(num(s), e === undefined ? undefined : num(e)),
  abbreviate: ([v, n]) => {
    const s = str(v);
    const max = num(n);
    return s.length <= max ? s : `${s.slice(0, Math.max(0, max - 3))}...`;
  },
  defaultIfEmpty: ([v, d]) => (truthy(v) ? v : d),
  default: ([v, d]) => (truthy(v) ? v : d),
  urlEncode: ([v], h) => (h.decode ? decodeURIComponent(str(v)) : encodeURIComponent(str(v))),
  base64: ([v], h) => {
    if (h.decode) {
      try {
        return new TextDecoder().decode(Uint8Array.from(atob(str(v)), (c) => c.charCodeAt(0)));
      } catch {
        throw new TemplateError("base64 decode: invalid input");
      }
    }
    const bytes = new TextEncoder().encode(str(v));
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    return btoa(bin);
  },
  // collections
  size: ([v]) => (v instanceof Multi ? v.values.length : Array.isArray(v) ? v.length : typeof v === "string" ? v.length : v && typeof v === "object" ? Object.keys(v).length : 0),
  array: (args) => args,
  range: ([a, b]) => {
    const lo = Math.trunc(num(a));
    const hi = Math.trunc(num(b));
    if (hi - lo > MAX_ITERATIONS) throw new TemplateError(`range is limited to ${MAX_ITERATIONS} items`);
    const out: number[] = [];
    for (let i = lo; i <= hi; i++) out.push(i);
    return out;
  },
  join: (args) => {
    // {{join ',' array}} or {{join array ','}} or {{join ',' 'a' 'b'}}
    if (args.length === 2 && (Array.isArray(args[0]) || args[0] instanceof Multi)) return list(args[0]).map(str).join(str(args[1]));
    const [sep, ...rest] = args;
    const items = rest.length === 1 ? list(rest[0]) : rest;
    return items.map(str).join(str(sep));
  },
  pickRandom: (args) => {
    const items = args.length === 1 ? list(args[0]) : args;
    return items.length ? items[randomInt(0, items.length - 1)] : "";
  },
  // numbers / random
  math: ([a, op, b]) => {
    const x = num(a);
    const y = num(b);
    switch (str(op)) {
      case "+":
        return x + y;
      case "-":
        return x - y;
      case "*":
        return x * y;
      case "/":
        return y === 0 ? 0 : x / y;
      case "%":
        return y === 0 ? 0 : x % y;
      default:
        throw new TemplateError(`math: unsupported operator ${str(op)}`);
    }
  },
  randomInt: (_a, h) => randomInt(h.lower === undefined ? 0 : num(h.lower), h.upper === undefined ? 2 ** 31 - 1 : num(h.upper)),
  randomDecimal: (_a, h) => {
    const lo = h.lower === undefined ? 0 : num(h.lower);
    const hi = h.upper === undefined ? 1 : num(h.upper);
    return lo + Math.random() * (hi - lo);
  },
  randomValue: (_a, h) => {
    const type = str(h.type ?? "ALPHANUMERIC").toUpperCase();
    if (type === "UUID") return uuid();
    const set = RANDOM_SETS[type];
    if (!set) throw new TemplateError(`randomValue: unknown type ${type}`);
    const len = Math.min(4096, Math.max(0, Math.trunc(num(h.length ?? 36))));
    const s = randomString(len, set);
    return h.uppercase ? s.toUpperCase() : s;
  },
  random: ([k]) => {
    const f = FAKER[fakerKey(str(k))];
    if (!f) throw new TemplateError(`random: unsupported faker key "${str(k)}"`);
    return f();
  },
  // conditions
  eq: ([a, b]) => str(a) === str(b),
  neq: ([a, b]) => str(a) !== str(b),
  ne: ([a, b]) => str(a) !== str(b),
  gt: ([a, b]) => cmp(a, b) > 0,
  gte: ([a, b]) => cmp(a, b) >= 0,
  lt: ([a, b]) => cmp(a, b) < 0,
  lte: ([a, b]) => cmp(a, b) <= 0,
  and: (args) => args.every(truthy),
  or: (args) => args.some(truthy),
  not: ([v]) => !truthy(v),
  contains: ([a, b]) => (Array.isArray(a) || a instanceof Multi ? list(a).map(str).includes(str(b)) : str(a).includes(str(b))),
  matches: ([a, b]) => {
    try {
      return new RegExp(`^(?:${str(b)})$`).test(str(a));
    } catch {
      throw new TemplateError(`matches: invalid regex ${str(b)}`);
    }
  },
  // JSON
  jsonPath: ([doc, path], h) => {
    const json = asJson(doc);
    try {
      const vals = queryJsonPath(json, str(path));
      const v = isDefinite(str(path)) ? vals[0] : vals;
      if (v === undefined || (Array.isArray(v) && !v.length && !isDefinite(str(path)))) return h.default ?? "";
      return v;
    } catch (e) {
      if (e instanceof JsonPathError) throw new TemplateError(`jsonPath: ${e.message}`);
      throw e;
    }
  },
  toJson: ([v]) => JSON.stringify(v instanceof Multi ? v.values : v),
  formatJson: ([v]) => JSON.stringify(asJson(v), null, 2),
  // dates
  now: (_a, h, st) => {
    let ms = st.now();
    if (h.offset !== undefined) ms = applyOffset(ms, str(h.offset));
    return formatDate(ms, h.format === undefined ? undefined : str(h.format));
  },
};
