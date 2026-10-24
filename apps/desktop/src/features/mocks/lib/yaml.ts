/**
 * A compact YAML reader/writer for what this module needs: `mocks/servers.yaml` and OpenAPI /
 * Swagger specs. It understands block mappings and sequences, flow collections (`{a: 1}`,
 * `[a, b]`), quoted and plain scalars, block scalars (`|`, `>`) and comments. Anchors, aliases and
 * multi-document streams are rejected. The desktop app prefers the Rust YAML parser
 * (`yaml.toJson` utility) and falls back to this one.
 */

export class YamlError extends Error {}

interface Line {
  indent: number;
  text: string;
  no: number;
}

function stripComment(s: string): string {
  let q: string | null = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === q) {
        if (q === "'" && s[i + 1] === "'") i++;
        else q = null;
      } else if (c === '\\' && q === '"') i++;
      continue;
    }
    if ((c === '"' || c === "'") && (i === 0 || /[\s:[{,-]/.test(s[i - 1]))) q = c;
    else if (c === '#' && (i === 0 || /\s/.test(s[i - 1]))) return s.slice(0, i).trimEnd();
  }
  return s.trimEnd();
}

const NUM = /^[-+]?(?:\d+|\d*\.\d+|\d+\.\d*)(?:[eE][-+]?\d+)?$/;

export function plainScalar(raw: string): unknown {
  const s = raw.trim();
  if (s === '' || s === '~' || /^null$/i.test(s)) return null;
  if (/^(true|false)$/i.test(s)) return s.toLowerCase() === 'true';
  if (/^0x[0-9a-f]+$/i.test(s)) return parseInt(s, 16);
  if (/^[-+]?\.inf$/i.test(s)) return s.startsWith('-') ? -Infinity : Infinity;
  if (/^\.nan$/i.test(s)) return NaN;
  if (NUM.test(s) && !/^[-+]?0\d/.test(s)) return Number(s);
  if (s.startsWith('&') || s.startsWith('*')) throw new YamlError(`YAML anchors and aliases are not supported (${s.split(/\s/)[0]})`);
  return s;
}

function unquote(s: string): string {
  if (s.startsWith("'")) {
    if (!s.endsWith("'") || s.length < 2) throw new YamlError(`Unterminated string: ${s}`);
    return s.slice(1, -1).replace(/''/g, "'");
  }
  if (!s.endsWith('"') || s.length < 2) throw new YamlError(`Unterminated string: ${s}`);
  try {
    return JSON.parse(s.replace(/\\x([0-9a-fA-F]{2})/g, '\\u00$1').replace(/\\([0abtnvfre /])/g, (m, c) => (c === '/' ? '/' : c === ' ' ? ' ' : c === 'e' ? '\\u001b' : c === '0' ? '\\u0000' : c === 'a' ? '\\u0007' : c === 'v' ? '\\u000b' : m)));
  } catch {
    throw new YamlError(`Invalid double-quoted string: ${s}`);
  }
}

function scalar(raw: string): unknown {
  let s = raw.trim();
  // Tags like !!str / !!int: drop them (values are inferred anyway).
  const tag = /^!!(\w+)\s+/.exec(s);
  if (tag) {
    s = s.slice(tag[0].length);
    if (tag[1] === 'str') return s.startsWith('"') || s.startsWith("'") ? unquote(s) : s;
  }
  if (s.startsWith('"') || s.startsWith("'")) return unquote(s);
  return plainScalar(s);
}

// ---------------------------------------------------------------- flow collections

class Flow {
  i = 0;
  constructor(readonly s: string) {}
  ws() {
    while (this.i < this.s.length && /\s/.test(this.s[this.i])) this.i++;
  }
  value(): unknown {
    this.ws();
    const c = this.s[this.i];
    if (c === '{') return this.map();
    if (c === '[') return this.seq();
    if (c === '"' || c === "'") return this.quoted();
    return scalar(this.plain(false));
  }
  quoted(): string {
    const q = this.s[this.i];
    let j = this.i + 1;
    for (; j < this.s.length; j++) {
      if (this.s[j] === '\\' && q === '"') j++;
      else if (this.s[j] === q) {
        if (q === "'" && this.s[j + 1] === "'") j++;
        else break;
      }
    }
    const out = unquote(this.s.slice(this.i, j + 1));
    this.i = j + 1;
    return out;
  }
  plain(key: boolean): string {
    const start = this.i;
    while (this.i < this.s.length) {
      const c = this.s[this.i];
      if (c === ',' || c === ']' || c === '}') break;
      if (key && c === ':' && (this.i + 1 >= this.s.length || /[\s,\]}]/.test(this.s[this.i + 1]))) break;
      this.i++;
    }
    return this.s.slice(start, this.i).trim();
  }
  map(): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    this.i++;
    for (;;) {
      this.ws();
      if (this.s[this.i] === '}') {
        this.i++;
        return out;
      }
      const c = this.s[this.i];
      const key = c === '"' || c === "'" ? this.quoted() : this.plain(true);
      this.ws();
      let val: unknown = null;
      if (this.s[this.i] === ':') {
        this.i++;
        this.ws();
        val = this.s[this.i] === ',' || this.s[this.i] === '}' ? null : this.value();
      }
      out[String(key)] = val;
      this.ws();
      if (this.s[this.i] === ',') this.i++;
      else if (this.s[this.i] !== '}') throw new YamlError(`Expected , or } in ${this.s}`);
    }
  }
  seq(): unknown[] {
    const out: unknown[] = [];
    this.i++;
    for (;;) {
      this.ws();
      if (this.s[this.i] === ']') {
        this.i++;
        return out;
      }
      out.push(this.value());
      this.ws();
      if (this.s[this.i] === ',') this.i++;
      else if (this.s[this.i] !== ']') throw new YamlError(`Expected , or ] in ${this.s}`);
    }
  }
}

function balanced(s: string): boolean {
  let depth = 0;
  let q: string | null = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === '\\' && q === '"') i++;
      else if (c === q) q = null;
      continue;
    }
    if (c === '"' || c === "'") q = c;
    else if (c === '{' || c === '[') depth++;
    else if (c === '}' || c === ']') depth--;
  }
  return depth <= 0;
}

// ---------------------------------------------------------------- block structure

/** Position of the `:` that separates a mapping key from its value, or -1. */
function keyColon(s: string): number {
  let q: string | null = null;
  let depth = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === '\\' && q === '"') i++;
      else if (c === q) q = null;
      continue;
    }
    if ((c === '"' || c === "'") && i === 0) q = c;
    else if (c === '{' || c === '[') depth++;
    else if (c === '}' || c === ']') depth--;
    else if (c === ':' && depth === 0 && (i + 1 === s.length || s[i + 1] === ' ' || s[i + 1] === '\t')) return i;
  }
  return -1;
}

const isSeqItem = (t: string) => t === '-' || t.startsWith('- ');

class Parser {
  i = 0;
  lines: Line[] = [];
  constructor(readonly raw: string[]) {}

  /** Next meaningful line (skips blank and comment-only lines) without consuming it. */
  peek(): Line | undefined {
    while (this.i < this.raw.length) {
      const r = this.raw[this.i];
      const text = stripComment(r);
      if (!text.trim()) {
        this.i++;
        continue;
      }
      if (text.trim() === '---') {
        this.i++;
        continue;
      }
      if (text.trim() === '...') {
        this.i = this.raw.length;
        return undefined;
      }
      if (/^\t/.test(r)) throw new YamlError(`Line ${this.i + 1}: tabs are not allowed for indentation`);
      const indent = text.length - text.trimStart().length;
      return { indent, text: text.trim(), no: this.i };
    }
    return undefined;
  }

  block(minIndent: number): unknown {
    const l = this.peek();
    if (!l || l.indent < minIndent) return null;
    if (isSeqItem(l.text)) return this.seq(l.indent);
    if (keyColon(l.text) >= 0 && !(l.text.startsWith('{') || l.text.startsWith('['))) return this.map(l.indent);
    // A lone scalar or flow collection (possibly spanning lines).
    this.i = l.no + 1;
    return this.inline(l.text);
  }

  inline(text: string): unknown {
    let s = text;
    if (s.startsWith('{') || s.startsWith('[')) {
      while (!balanced(s) && this.i < this.raw.length) s += ' ' + stripComment(this.raw[this.i++]).trim();
      const f = new Flow(s);
      const v = f.value();
      return v;
    }
    return scalar(s);
  }

  blockScalar(header: string, parentIndent: number): string {
    const fold = header.startsWith('>');
    const chomp = header.includes('-') ? 'strip' : header.includes('+') ? 'keep' : 'clip';
    const lines: string[] = [];
    let indent = -1;
    while (this.i < this.raw.length) {
      const r = this.raw[this.i];
      if (!r.trim()) {
        lines.push('');
        this.i++;
        continue;
      }
      const ind = r.length - r.trimStart().length;
      if (ind <= parentIndent) break;
      if (indent < 0) indent = ind;
      if (ind < indent) break;
      lines.push(r.slice(indent));
      this.i++;
    }
    let trailing = 0;
    while (lines.length && lines[lines.length - 1] === '') {
      lines.pop();
      trailing++;
    }
    let body = fold ? lines.reduce((acc, l, k) => (k === 0 ? l : acc + (l === '' || lines[k - 1] === '' || /^\s/.test(l) ? '\n' : ' ') + l), '') : lines.join('\n');
    if (fold) body = body.replace(/\n\n/g, '\n');
    if (chomp === 'strip') return body;
    if (chomp === 'keep') return body + '\n'.repeat(trailing + 1);
    return body ? body + '\n' : body;
  }

  /** The value after `key:` on a line at `indent`. */
  valueAfter(rest: string, indent: number, inSeqMap: boolean): unknown {
    const v = rest.trim();
    if (v === '') {
      const next = this.peek();
      if (!next) return null;
      if (next.indent > indent) return this.block(next.indent);
      // "key:\n- a" — a sequence at the key's own indentation belongs to the key.
      if (next.indent === indent && isSeqItem(next.text) && !inSeqMap) return this.seq(indent);
      return null;
    }
    if (/^[|>][-+0-9]*$/.test(v)) return this.blockScalar(v, indent);
    if (v.startsWith('&') || v.startsWith('*')) throw new YamlError(`Line ${this.i}: YAML anchors and aliases are not supported`);
    return this.inline(v);
  }

  map(indent: number, inSeqMap = false): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (;;) {
      const l = this.peek();
      if (!l || l.indent < indent) break;
      if (l.indent > indent) throw new YamlError(`Line ${l.no + 1}: unexpected indentation`);
      if (isSeqItem(l.text)) break;
      const c = keyColon(l.text);
      if (c < 0) throw new YamlError(`Line ${l.no + 1}: expected "key: value", got "${l.text}"`);
      const rawKey = l.text.slice(0, c).trim();
      const key = rawKey.startsWith('"') || rawKey.startsWith("'") ? unquote(rawKey) : rawKey;
      if (key === '<<') throw new YamlError(`Line ${l.no + 1}: merge keys are not supported`);
      this.i = l.no + 1;
      out[key] = this.valueAfter(l.text.slice(c + 1), indent, inSeqMap);
    }
    return out;
  }

  seq(indent: number): unknown[] {
    const out: unknown[] = [];
    for (;;) {
      const l = this.peek();
      if (!l || l.indent !== indent || !isSeqItem(l.text)) break;
      const rest = l.text === '-' ? '' : l.text.slice(2);
      const restTrim = rest.trimStart();
      const col = indent + 2 + (rest.length - restTrim.length);
      if (!restTrim) {
        this.i = l.no + 1;
        const next = this.peek();
        out.push(next && next.indent > indent ? this.block(next.indent) : null);
        continue;
      }
      if (isSeqItem(restTrim) || (keyColon(restTrim) >= 0 && !restTrim.startsWith('{') && !restTrim.startsWith('['))) {
        // Re-read the item content as a block at its own column ("- key: v" / "- - x").
        this.raw[l.no] = ' '.repeat(col) + restTrim;
        out.push(isSeqItem(restTrim) ? this.seq(col) : this.map(col, true));
        continue;
      }
      this.i = l.no + 1;
      if (/^[|>][-+0-9]*$/.test(restTrim)) out.push(this.blockScalar(restTrim, indent));
      else out.push(this.inline(restTrim));
    }
    return out;
  }
}

export function parseYaml(text: string): unknown {
  const p = new Parser(text.replace(/^﻿/, '').split(/\r?\n/));
  const first = p.peek();
  if (!first) return null;
  const v = p.block(first.indent);
  const rest = p.peek();
  if (rest) throw new YamlError(`Line ${rest.no + 1}: unexpected content "${rest.text}"`);
  return v;
}

// ---------------------------------------------------------------- writer

function needsQuotes(s: string): boolean {
  if (s === '') return true;
  if (s !== s.trim()) return true;
  if (/[\n\r\t]/.test(s)) return true;
  if (/^[-?:,[\]{}#&*!|>'"%@`]/.test(s)) return true;
  if (/: |\s#|:$/.test(s)) return true;
  const v = plainScalarSafe(s);
  return typeof v !== 'string';
}

function plainScalarSafe(s: string): unknown {
  try {
    return plainScalar(s);
  } catch {
    return null;
  }
}

function str(s: string): string {
  return needsQuotes(s) ? JSON.stringify(s) : s;
}

function key(k: string): string {
  return needsQuotes(k) || k.includes(':') ? JSON.stringify(k) : k;
}

function scalarOut(v: unknown): string {
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'string') return str(v);
  if (typeof v === 'number') return Number.isFinite(v) ? String(v) : Number.isNaN(v) ? '.nan' : v > 0 ? '.inf' : '-.inf';
  if (typeof v === 'boolean') return String(v);
  return JSON.stringify(v);
}

const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);

function emit(v: unknown, indent: number, out: string[]) {
  const pad = ' '.repeat(indent);
  if (Array.isArray(v)) {
    if (!v.length) {
      out[out.length - 1] += ' []';
      return;
    }
    for (const item of v) {
      if (isObj(item) && Object.keys(item).length) {
        const lines: string[] = [''];
        emit(item, indent + 2, lines);
        lines.shift();
        out.push(`${pad}- ${lines[0].slice(indent + 2)}`, ...lines.slice(1));
      } else if (Array.isArray(item) && item.length) {
        out.push(`${pad}-`);
        emit(item, indent + 2, out);
      } else out.push(`${pad}- ${isObj(item) ? '{}' : Array.isArray(item) ? '[]' : scalarOut(item)}`);
    }
    return;
  }
  if (isObj(v)) {
    const entries = Object.entries(v).filter(([, x]) => x !== undefined);
    if (!entries.length) {
      out[out.length - 1] += ' {}';
      return;
    }
    for (const [k, x] of entries) {
      if ((isObj(x) && Object.keys(x).length) || (Array.isArray(x) && x.length)) {
        out.push(`${pad}${key(k)}:`);
        emit(x, Array.isArray(x) ? indent + 2 : indent + 2, out);
      } else out.push(`${pad}${key(k)}: ${isObj(x) ? '{}' : Array.isArray(x) ? '[]' : scalarOut(x)}`);
    }
    return;
  }
  out.push(pad + scalarOut(v));
}

/** Block-style YAML for plain data (objects, arrays, scalars). `undefined` values are skipped. */
export function stringifyYaml(v: unknown): string {
  if (!isObj(v) && !Array.isArray(v)) return scalarOut(v) + '\n';
  if (Array.isArray(v) ? !v.length : !Object.values(v).some((x) => x !== undefined)) return Array.isArray(v) ? '[]\n' : '{}\n';
  const out: string[] = [''];
  emit(v, 0, out);
  out.shift();
  return out.join('\n') + '\n';
}
