// nbformat 4 notebooks in the editor: parse, edit and write back without losing anything the
// file had (unknown notebook/cell fields, key order, indentation), so files stay round-trip safe
// with Jupyter and VS Code.
import { normalizeLanguage, type MimeBundle, type NbOutput, type RunLanguage } from '@/lib/api/notebooks';
import { stripAnsi } from './ansi';

export type CellType = 'code' | 'markdown' | 'raw';
type Json = Record<string, unknown>;

export interface EditCell {
  id: string;
  type: CellType;
  source: string;
  /** The cell object as read (metadata, outputs, attachments, unknown fields). */
  raw: Json;
}

export interface NotebookDoc {
  /** Top-level object as read, `cells` included (keeps key order). */
  top: Json;
  indent: number;
  trailingNewline: boolean;
}

/** Output shown in the UI (raw text keeps ANSI escapes). */
export type UiOutput =
  | { kind: 'stream'; name: 'stdout' | 'stderr'; text: string }
  | { kind: 'display'; data: MimeBundle; result: boolean; executionCount?: number | null }
  | { kind: 'error'; ename: string; evalue: string; traceback: string[] }
  | { kind: 'notice'; text: string };

export const joinSource = (s: unknown): string => (Array.isArray(s) ? s.filter((x) => typeof x === 'string').join('') : typeof s === 'string' ? s : '');

/** nbformat multiline string: lines keeping their `\n` (Jupyter's on-disk convention). */
export const toLines = (s: string): string[] => (s ? s.split(/(?<=\n)/) : []);

/** FNV-1a, base 36 (stable short keys, not security). */
export function shortHash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(36);
}

export function newCellId(): string {
  const bytes = new Uint8Array(4);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function parseNotebook(text: string): { doc: NotebookDoc; cells: EditCell[] } {
  const top = JSON.parse(text) as Json;
  if (top.nbformat !== 4 || !Array.isArray(top.cells)) throw new Error('Only nbformat 4 notebooks are supported.');
  const m = /\n( +)"/.exec(text);
  const indent = m ? m[1].length : 1;
  const seen = new Set<string>();
  const cells = (top.cells as Json[]).map((raw) => {
    let id = typeof raw.id === 'string' && raw.id ? raw.id : newCellId();
    while (seen.has(id)) id = newCellId();
    seen.add(id);
    const t = raw.cell_type;
    return { id, type: (t === 'markdown' || t === 'raw' ? t : 'code') as CellType, source: joinSource(raw.source), raw: { ...raw, metadata: (raw.metadata as Json) ?? {} } };
  });
  return { doc: { top, indent, trailingNewline: text.endsWith('\n') }, cells };
}

const meta = (o: unknown, ...path: string[]): unknown => path.reduce<unknown>((v, k) => (v && typeof v === 'object' ? (v as Json)[k] : undefined), o);

/** The notebook's kernel language (`language_info.name`, else `kernelspec.language`). */
export function kernelLanguage(top: Json): RunLanguage | null {
  const li = meta(top, 'metadata', 'language_info', 'name');
  const v = li !== undefined ? li : meta(top, 'metadata', 'kernelspec', 'language');
  return typeof v === 'string' ? normalizeLanguage(v) : null;
}

/**
 * A code cell's language and the code to run — same rules as `devall_core::notebook::cell_language`:
 * a `%%bash`-style magic on the first line wins (and is stripped), then `metadata.devall.language`,
 * then `metadata.vscode.languageId`, then the kernel language.
 */
export function cellLanguage(cell: EditCell, top: Json): { language: RunLanguage | null; code: string; magic: boolean } {
  const src = cell.source;
  const first = src.split('\n')[0].trim();
  if (first.startsWith('%%')) {
    const lang = normalizeLanguage(first.slice(2).split(/\s+/)[0] ?? '');
    if (lang) {
      const nl = src.indexOf('\n');
      return { language: lang, code: nl >= 0 ? src.slice(nl + 1) : '', magic: true };
    }
  }
  const dl = meta(cell.raw.metadata, 'devall', 'language');
  const explicitRaw = dl !== undefined ? dl : meta(cell.raw.metadata, 'vscode', 'languageId');
  const explicit = typeof explicitRaw === 'string' ? normalizeLanguage(explicitRaw) : null;
  return { language: explicit ?? kernelLanguage(top), code: src, magic: false };
}

const VSCODE_IDS: Record<RunLanguage, string> = { shell: 'shellscript', powershell: 'powershell', cmd: 'bat', javascript: 'javascript', python: 'python' };

/** Sets a code cell's language (`metadata.devall.language`; a magic line is removed). */
export function withLanguage(cell: EditCell, language: RunLanguage): EditCell {
  let source = cell.source;
  const first = source.split('\n')[0].trim();
  if (first.startsWith('%%') && normalizeLanguage(first.slice(2).split(/\s+/)[0] ?? '')) {
    const nl = source.indexOf('\n');
    source = nl >= 0 ? source.slice(nl + 1) : '';
  }
  const md = { ...(cell.raw.metadata as Json) };
  md.devall = { ...((md.devall as Json) ?? {}), language };
  if (md.vscode && typeof md.vscode === 'object') md.vscode = { ...(md.vscode as Json), languageId: VSCODE_IDS[language] };
  return { ...cell, source, raw: { ...cell.raw, metadata: md } };
}

export const devallMeta = (cell: EditCell): Json => ((cell.raw.metadata as Json)?.devall as Json) ?? {};

export function withDevallMeta(cell: EditCell, patch: Json): EditCell {
  const md = { ...(cell.raw.metadata as Json) };
  const next: Json = { ...((md.devall as Json) ?? {}), ...patch };
  for (const k of Object.keys(next)) if (next[k] === undefined) delete next[k];
  if (Object.keys(next).length) md.devall = next;
  else delete md.devall;
  return { ...cell, raw: { ...cell.raw, metadata: md } };
}

export function newCell(type: CellType, language?: RunLanguage | null): EditCell {
  const id = newCellId();
  const raw: Json =
    type === 'code'
      ? { cell_type: 'code', execution_count: null, id, metadata: language ? { devall: { language } } : {}, outputs: [], source: [] }
      : { cell_type: type, id, metadata: {}, source: [] };
  return { id, type, source: '', raw };
}

/** Converts saved nbformat outputs for display. */
export function fromNbOutputs(outputs: unknown): UiOutput[] {
  if (!Array.isArray(outputs)) return [];
  const out: UiOutput[] = [];
  for (const o of outputs as Json[]) {
    switch (o.output_type) {
      case 'stream':
        out.push({ kind: 'stream', name: o.name === 'stderr' ? 'stderr' : 'stdout', text: joinSource(o.text) });
        break;
      case 'execute_result':
      case 'display_data': {
        const data: MimeBundle = {};
        for (const [k, v] of Object.entries((o.data as Json) ?? {})) data[k] = Array.isArray(v) && (k.startsWith('text/') || k === 'image/svg+xml') ? joinSource(v) : v;
        out.push({ kind: 'display', data, result: o.output_type === 'execute_result', executionCount: (o.execution_count as number | null) ?? null });
        break;
      }
      case 'error':
        out.push({ kind: 'error', ename: String(o.ename ?? ''), evalue: String(o.evalue ?? ''), traceback: Array.isArray(o.traceback) ? (o.traceback as string[]) : [] });
        break;
    }
  }
  return out;
}

/** UI outputs → nbformat outputs (ANSI stripped, text split into lines; notices dropped). */
export function toNbOutputs(outputs: UiOutput[], executionCount: number | null): NbOutput[] {
  const res: NbOutput[] = [];
  for (const o of outputs) {
    if (o.kind === 'stream') {
      const text = stripAnsi(o.text);
      const prev = res[res.length - 1];
      if (prev && prev.output_type === 'stream' && prev.name === o.name) prev.text = toLines(joinSource(prev.text) + text);
      else if (text) res.push({ output_type: 'stream', name: o.name, text: toLines(text) });
    } else if (o.kind === 'display') {
      const data: MimeBundle = {};
      for (const [k, v] of Object.entries(o.data)) data[k] = typeof v === 'string' && (k.startsWith('text/') || k === 'image/svg+xml') ? toLines(k === 'text/plain' ? stripAnsi(v) : v) : v;
      if (o.result) res.push({ output_type: 'execute_result', execution_count: o.executionCount ?? executionCount, data, metadata: {} });
      else res.push({ output_type: 'display_data', data, metadata: {} });
    } else if (o.kind === 'error') {
      res.push({ output_type: 'error', ename: o.ename, evalue: stripAnsi(o.evalue), traceback: o.traceback.map(stripAnsi) });
    }
  }
  return res;
}

export interface SerializeOptions {
  saveOutputs: boolean;
  /** Outputs of cells run (or cleared) this session; `undefined` keeps the file's outputs. */
  outputs(cellId: string): { outputs: UiOutput[]; executionCount: number | null } | undefined;
}

/** Writes the notebook back, preserving unknown fields, key order and indentation. */
export function serializeNotebook(doc: NotebookDoc, cells: EditCell[], opts: SerializeOptions): string {
  let addedIds = false;
  const outCells = cells.map((c) => {
    const o: Json = { ...c.raw };
    o.cell_type = c.type;
    if (o.id !== c.id) {
      addedIds = addedIds || o.id === undefined;
      o.id = c.id;
    }
    o.metadata = (c.raw.metadata as Json) ?? {};
    o.source = toLines(c.source);
    if (c.type === 'code') {
      const cur = opts.saveOutputs ? opts.outputs(c.id) : undefined;
      if (!opts.saveOutputs) {
        o.outputs = [];
        o.execution_count = null;
      } else if (cur) {
        o.outputs = toNbOutputs(cur.outputs, cur.executionCount);
        o.execution_count = cur.outputs.length ? cur.executionCount : null;
      } else {
        // Not run (or cleared) this session: keep what the file had, byte for byte.
        o.outputs = Array.isArray(c.raw.outputs) ? c.raw.outputs : [];
        o.execution_count = c.raw.execution_count ?? null;
      }
    } else {
      delete o.outputs;
      delete o.execution_count;
    }
    return o;
  });
  const top: Json = { ...doc.top, cells: outCells };
  if (addedIds && typeof top.nbformat_minor === 'number' && top.nbformat_minor < 5) top.nbformat_minor = 5;
  return JSON.stringify(top, null, doc.indent) + (doc.trailingNewline ? '\n' : '');
}

/** Monaco language for a run language. */
export function monacoLanguage(l: RunLanguage | null): 'shell' | 'powershell' | 'javascript' | 'python' | 'plaintext' {
  switch (l) {
    case 'shell':
      return 'shell';
    case 'powershell':
      return 'powershell';
    case 'javascript':
      return 'javascript';
    case 'python':
      return 'python';
    default:
      return 'plaintext';
  }
}
