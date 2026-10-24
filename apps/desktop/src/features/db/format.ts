import type { ColumnMeta, Dialect } from '@/lib/api/db';

/** Display text of a cell. `null` is rendered separately (dimmed NULL). */
export function cellText(v: unknown): string {
  if (v === null || v === undefined) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean' || typeof v === 'bigint') return String(v);
  return JSON.stringify(v);
}

/** First line, cut for the grid. */
export function cellPreview(v: unknown, max = 400): string {
  const t = cellText(v);
  const nl = t.indexOf('\n');
  const line = nl >= 0 ? `${t.slice(0, nl)} ↵` : t;
  return line.length > max ? `${line.slice(0, max)}…` : line;
}

const NUMERIC = /^-?\d+(\.\d+)?([eE][-+]?\d+)?$/;

/** Sort comparator: NULLs last, numbers (incl. big-number strings) numerically, then text. */
export function compareValues(a: unknown, b: unknown): number {
  const an = a === null || a === undefined;
  const bn = b === null || b === undefined;
  if (an || bn) return an === bn ? 0 : an ? 1 : -1;
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  if (typeof a === 'boolean' && typeof b === 'boolean') return Number(a) - Number(b);
  const as = cellText(a);
  const bs = cellText(b);
  if (NUMERIC.test(as) && NUMERIC.test(bs)) {
    const d = Number(as) - Number(bs);
    if (d !== 0 && Number.isFinite(d)) return d;
  }
  return as.localeCompare(bs, undefined, { numeric: true, sensitivity: 'base' });
}

export function quoteIdent(name: string, d: Dialect): string {
  if (d === 'sqlserver') return `[${name.split(']').join(']]')}]`;
  const q = d === 'mysql' ? '`' : '"';
  return q + name.split(q).join(q + q) + q;
}

export function sqlLiteral(v: unknown, d: Dialect): string {
  if (v === null || v === undefined) return 'NULL';
  if (typeof v === 'boolean') return d === 'sqlite' || d === 'sqlserver' ? (v ? '1' : '0') : v ? 'TRUE' : 'FALSE';
  if (typeof v === 'number') return String(v);
  const s = typeof v === 'string' ? v : JSON.stringify(v);
  let out = s.split("'").join("''");
  if (d === 'mysql') out = out.split('\\').join('\\\\');
  return d === 'sqlserver' ? `N'${out}'` : `'${out}'`;
}

/** `SELECT … LIMIT n` / `SELECT TOP (n) …` (T-SQL has no LIMIT). */
export function selectSql(columns: string, from: string, limit: number, d: Dialect): string {
  return d === 'sqlserver' ? `SELECT TOP (${limit}) ${columns}\nFROM ${from};\n` : `SELECT ${columns}\nFROM ${from}\nLIMIT ${limit};\n`;
}

function csvField(v: unknown): string {
  if (v === null || v === undefined) return '';
  const t = cellText(v);
  return /[",\n\r]/.test(t) ? `"${t.split('"').join('""')}"` : t;
}

export function toCsv(columns: ColumnMeta[], rows: unknown[][]): string {
  const lines = [columns.map((c) => csvField(c.name)).join(',')];
  for (const r of rows) lines.push(r.map(csvField).join(','));
  return lines.join('\n') + '\n';
}

export function toTsv(columns: ColumnMeta[] | null, rows: unknown[][]): string {
  const clean = (v: unknown) => cellText(v).replace(/[\t\n\r]/g, ' ');
  const lines = columns ? [columns.map((c) => c.name).join('\t')] : [];
  for (const r of rows) lines.push(r.map(clean).join('\t'));
  return lines.join('\n');
}

export function toObjects(columns: ColumnMeta[], rows: unknown[][]): Record<string, unknown>[] {
  return rows.map((r) => {
    const o: Record<string, unknown> = {};
    columns.forEach((c, i) => {
      let name = c.name;
      let n = 2;
      while (name in o) name = `${c.name}_${n++}`;
      o[name] = r[i];
    });
    return o;
  });
}

export function toJson(columns: ColumnMeta[], rows: unknown[][]): string {
  return JSON.stringify(toObjects(columns, rows), null, 2) + '\n';
}

export function toInsert(table: string, columns: ColumnMeta[], rows: unknown[][], d: Dialect): string {
  const cols = columns.map((c) => quoteIdent(c.name, d)).join(', ');
  return rows.map((r) => `INSERT INTO ${table} (${cols}) VALUES (${r.map((v) => sqlLiteral(v, d)).join(', ')});`).join('\n') + '\n';
}

export function toMarkdown(columns: ColumnMeta[], rows: unknown[][]): string {
  const esc = (v: unknown) => (v === null || v === undefined ? 'NULL' : cellPreview(v, 200).split('|').join('\\|'));
  const head = `| ${columns.map((c) => c.name.split('|').join('\\|')).join(' | ')} |`;
  const sep = `| ${columns.map(() => '---').join(' | ')} |`;
  return [head, sep, ...rows.map((r) => `| ${r.map(esc).join(' | ')} |`)].join('\n') + '\n';
}

export function fmtMs(ms: number): string {
  if (ms < 1) return '<1 ms';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 2 : 1)} s`;
  return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
}

export function fmtCount(n: number): string {
  return n.toLocaleString();
}

export function fmtTtl(ms: number): string {
  if (ms === -1) return 'no TTL';
  if (ms === -2) return 'expired';
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`;
}

/** Decodes base64 into a classic hex dump (offset, hex, ASCII). */
export function hexDump(b64: string, maxBytes = 64 * 1024): string {
  let bin: string;
  try {
    bin = atob(b64);
  } catch {
    return '';
  }
  const n = Math.min(bin.length, maxBytes);
  const lines: string[] = [];
  for (let off = 0; off < n; off += 16) {
    const chunk = bin.slice(off, Math.min(off + 16, n));
    const hex = Array.from(chunk, (c) => c.charCodeAt(0).toString(16).padStart(2, '0')).join(' ');
    const ascii = Array.from(chunk, (c) => {
      const code = c.charCodeAt(0);
      return code >= 32 && code < 127 ? c : '.';
    }).join('');
    lines.push(`${off.toString(16).padStart(8, '0')}  ${hex.padEnd(47)}  ${ascii}`);
  }
  if (bin.length > n) lines.push(`… ${bin.length - n} more bytes`);
  return lines.join('\n');
}

/** Pretty JSON when the text parses as an object/array; otherwise null. */
export function tryPrettyJson(text: string): string | null {
  const t = text.trim();
  if (!(t.startsWith('{') || t.startsWith('['))) return null;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return null;
  }
}
