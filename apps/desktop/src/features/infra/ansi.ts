import type { CSSProperties } from 'react';

// A small terminal model for exec sessions and colored logs: SGR colors/attributes, carriage
// return / backspace / tab, erase-in-line/display, cursor moves within the visible screen,
// insert/delete chars (enough for shells with readline). Full-screen TUIs (vim, top, k9s) need a
// real emulator (xterm.js); this deliberately stays dependency-free.

export interface CellStyle {
  fg?: string;
  bg?: string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
}

export interface TermLine {
  chars: string[];
  styles: number[];
  /** Bumped on every change, for memoized rendering. */
  v: number;
}

/** The 16 base colors as theme tokens (see infra.css). */
const BASE = Array.from({ length: 16 }, (_, i) => `var(--term-${i})`);

/** xterm 256-color palette: 0–15 theme tokens, 16–231 color cube, 232–255 grays. */
export function color256(n: number): string {
  if (n < 16) return BASE[n];
  if (n < 232) {
    const i = n - 16;
    const steps = [0, 95, 135, 175, 215, 255];
    return `rgb(${steps[Math.floor(i / 36)]}, ${steps[Math.floor(i / 6) % 6]}, ${steps[i % 6]})`;
  }
  const g = 8 + (n - 232) * 10;
  return `rgb(${g}, ${g}, ${g})`;
}

const styleIds = new Map<string, number>();
const styleList: CellStyle[] = [{}];
styleIds.set('{}', 0);

export function styleOf(id: number): CellStyle {
  return styleList[id] ?? {};
}

function internStyle(s: CellStyle): number {
  const clean: CellStyle = {};
  for (const k of ['fg', 'bg', 'bold', 'dim', 'italic', 'underline', 'inverse'] as const) if (s[k]) (clean as Record<string, unknown>)[k] = s[k];
  const key = JSON.stringify(clean);
  let id = styleIds.get(key);
  if (id === undefined) {
    id = styleList.length;
    styleList.push(clean);
    styleIds.set(key, id);
  }
  return id;
}

/** Applies SGR parameters to a style. */
export function applySgr(style: CellStyle, params: number[]): CellStyle {
  const s: CellStyle = { ...style };
  if (params.length === 0) params = [0];
  for (let i = 0; i < params.length; i++) {
    const p = params[i];
    if (p === 0) {
      for (const k of Object.keys(s)) delete (s as Record<string, unknown>)[k];
    } else if (p === 1) s.bold = true;
    else if (p === 2) s.dim = true;
    else if (p === 3) s.italic = true;
    else if (p === 4) s.underline = true;
    else if (p === 7) s.inverse = true;
    else if (p === 22) s.bold = s.dim = false;
    else if (p === 23) s.italic = false;
    else if (p === 24) s.underline = false;
    else if (p === 27) s.inverse = false;
    else if (p >= 30 && p <= 37) s.fg = BASE[p - 30];
    else if (p >= 90 && p <= 97) s.fg = BASE[p - 90 + 8];
    else if (p >= 40 && p <= 47) s.bg = BASE[p - 40];
    else if (p >= 100 && p <= 107) s.bg = BASE[p - 100 + 8];
    else if (p === 39) delete s.fg;
    else if (p === 49) delete s.bg;
    else if (p === 38 || p === 48) {
      const key = p === 38 ? 'fg' : 'bg';
      if (params[i + 1] === 5 && params[i + 2] !== undefined) {
        s[key] = color256(params[i + 2]);
        i += 2;
      } else if (params[i + 1] === 2 && params[i + 4] !== undefined) {
        s[key] = `rgb(${params[i + 2]}, ${params[i + 3]}, ${params[i + 4]})`;
        i += 4;
      }
    }
  }
  return s;
}

const newLine = (): TermLine => ({ chars: [], styles: [], v: 0 });

export class TermBuffer {
  lines: TermLine[] = [newLine()];
  row = 0;
  col = 0;
  /** Visible screen height, for cursor-addressed sequences. */
  screenRows = 24;
  bracketedPaste = false;
  appCursorKeys = false;
  cursorVisible = true;
  private style: CellStyle = {};
  private styleId = 0;
  private pending = '';
  private lineVersion = 0;
  /** Bumped when lines are dropped/cleared (forces a full re-render). */
  generation = 0;

  constructor(public maxLines = 5000) {}

  clear() {
    this.lines = [newLine()];
    this.row = 0;
    this.col = 0;
    this.generation++;
  }

  private touch(line: TermLine) {
    line.v = ++this.lineVersion;
  }

  private top(): number {
    return Math.max(0, this.lines.length - this.screenRows);
  }

  private ensureRow(r: number) {
    while (this.lines.length <= r) this.lines.push(newLine());
  }

  private newline() {
    this.row++;
    this.ensureRow(this.row);
    if (this.lines.length > this.maxLines) {
      const drop = this.lines.length - this.maxLines;
      this.lines.splice(0, drop);
      this.row -= drop;
      this.generation++;
    }
  }

  private put(ch: string) {
    const line = this.lines[this.row];
    while (line.chars.length < this.col) {
      line.chars.push(' ');
      line.styles.push(0);
    }
    line.chars[this.col] = ch;
    line.styles[this.col] = this.styleId;
    this.col++;
    this.touch(line);
  }

  private eraseInLine(mode: number) {
    const line = this.lines[this.row];
    if (mode === 0) {
      line.chars.length = Math.min(line.chars.length, this.col);
      line.styles.length = line.chars.length;
    } else if (mode === 1) {
      for (let i = 0; i <= this.col && i < line.chars.length; i++) {
        line.chars[i] = ' ';
        line.styles[i] = 0;
      }
    } else {
      line.chars = [];
      line.styles = [];
    }
    this.touch(line);
  }

  private csi(priv: string, params: number[], final: string) {
    const n = (i = 0, def = 1) => (params[i] === undefined || params[i] === 0 ? def : params[i]);
    const line = () => this.lines[this.row];
    switch (final) {
      case 'm':
        this.style = applySgr(this.style, params);
        this.styleId = internStyle(this.style);
        break;
      case 'K':
        this.eraseInLine(params[0] ?? 0);
        break;
      case 'J': {
        const mode = params[0] ?? 0;
        if (mode === 2 || mode === 3) this.clear();
        else if (mode === 0) {
          this.eraseInLine(0);
          if (this.lines.length > this.row + 1) {
            this.lines.length = this.row + 1;
            this.generation++;
          }
        }
        break;
      }
      case 'H':
      case 'f': {
        const r = this.top() + n(0) - 1;
        this.ensureRow(r);
        this.row = r;
        this.col = n(1) - 1;
        break;
      }
      case 'A':
        this.row = Math.max(this.top(), this.row - n());
        break;
      case 'B':
        this.row += n();
        this.ensureRow(this.row);
        break;
      case 'C':
        this.col += n();
        break;
      case 'D':
        this.col = Math.max(0, this.col - n());
        break;
      case 'E':
      case 'F':
        this.row = final === 'E' ? this.row + n() : Math.max(this.top(), this.row - n());
        this.ensureRow(this.row);
        this.col = 0;
        break;
      case 'G':
      case '`':
        this.col = n() - 1;
        break;
      case 'd': {
        const r = this.top() + n() - 1;
        this.ensureRow(r);
        this.row = r;
        break;
      }
      case 'P': {
        const l = line();
        l.chars.splice(this.col, n());
        l.styles.splice(this.col, n());
        this.touch(l);
        break;
      }
      case '@': {
        const l = line();
        if (this.col <= l.chars.length) {
          l.chars.splice(this.col, 0, ...Array(n()).fill(' '));
          l.styles.splice(this.col, 0, ...Array(n()).fill(0));
          this.touch(l);
        }
        break;
      }
      case 'X': {
        const l = line();
        for (let i = this.col; i < this.col + n() && i < l.chars.length; i++) {
          l.chars[i] = ' ';
          l.styles[i] = 0;
        }
        this.touch(l);
        break;
      }
      case 'h':
      case 'l':
        if (priv === '?') {
          const on = final === 'h';
          for (const p of params) {
            if (p === 2004) this.bracketedPaste = on;
            else if (p === 1) this.appCursorKeys = on;
            else if (p === 25) this.cursorVisible = on;
            else if ((p === 1049 || p === 1047 || p === 47) && on) this.clear();
          }
        }
        break;
      default:
        // Unsupported sequences are dropped.
        break;
    }
  }

  /** Feeds terminal output. Incomplete escape sequences carry over to the next call. */
  write(data: string) {
    let s = this.pending + data;
    this.pending = '';
    let i = 0;
    const len = s.length;
    while (i < len) {
      const ch = s[i];
      const code = ch.charCodeAt(0);
      if (ch === '\x1b') {
        if (i + 1 >= len) {
          this.pending = s.slice(i);
          return;
        }
        const next = s[i + 1];
        if (next === '[') {
          let j = i + 2;
          let priv = '';
          if (j < len && '?>=<'.includes(s[j])) priv = s[j++];
          const start = j;
          while (j < len && /[0-9;:]/.test(s[j])) j++;
          while (j < len && s.charCodeAt(j) >= 0x20 && s.charCodeAt(j) <= 0x2f) j++;
          if (j >= len) {
            this.pending = s.slice(i);
            return;
          }
          const final = s[j];
          const params = s
            .slice(start, j)
            .replace(/[ -/]+$/, '')
            .split(/[;:]/)
            .filter((p, _k, all) => p !== '' || all.length > 1)
            .map((p) => (p === '' ? 0 : Number(p)));
          this.csi(priv, params, final);
          i = j + 1;
          continue;
        }
        if (next === ']' || next === 'P' || next === '_' || next === '^') {
          // OSC/DCS/APC/PM: skip to BEL or ST.
          let j = i + 2;
          while (j < len && s[j] !== '\x07' && !(s[j] === '\x1b' && s[j + 1] === '\\')) j++;
          if (j >= len) {
            if (len - i > 4096) {
              i = len;
              continue;
            }
            this.pending = s.slice(i);
            return;
          }
          i = s[j] === '\x07' ? j + 1 : j + 2;
          continue;
        }
        if ('()*+'.includes(next)) {
          if (i + 2 >= len) {
            this.pending = s.slice(i);
            return;
          }
          i += 3;
          continue;
        }
        i += 2;
        continue;
      }
      if (ch === '\n') {
        this.newline();
        this.col = 0;
      } else if (ch === '\r') {
        this.col = 0;
      } else if (ch === '\b') {
        this.col = Math.max(0, this.col - 1);
      } else if (ch === '\t') {
        const to = (Math.floor(this.col / 8) + 1) * 8;
        while (this.col < to) this.put(' ');
      } else if (code >= 0x20 && code !== 0x7f) {
        this.put(ch);
      }
      i++;
    }
    s = '';
  }

  /** Plain text of the whole buffer (copy / open in log viewer). */
  text(): string {
    return this.lines.map((l) => l.chars.join('').trimEnd()).join('\n');
  }
}

/** Splits one line into same-style runs for rendering. */
export function runs(line: TermLine): { text: string; style: number }[] {
  const out: { text: string; style: number }[] = [];
  for (let i = 0; i < line.chars.length; i++) {
    const st = line.styles[i] ?? 0;
    const last = out[out.length - 1];
    if (last && last.style === st) last.text += line.chars[i];
    else out.push({ text: line.chars[i], style: st });
  }
  return out;
}

/** Strips escape sequences (for search and plain-text export). */
export function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;:?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(\x07|\x1b\\)|\x1b[()*+]?./g, '');
}

/** Parses one log line with SGR colors into runs (no cursor semantics). */
export function ansiRuns(text: string): { text: string; style: CellStyle }[] {
  if (!text.includes('\x1b')) return [{ text, style: {} }];
  const out: { text: string; style: CellStyle }[] = [];
  let style: CellStyle = {};
  // eslint-disable-next-line no-control-regex
  const re = /\x1b\[([0-9;:]*)m|\x1b\[[0-9;:?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push({ text: text.slice(last, m.index), style });
    if (m[1] !== undefined) style = applySgr(style, m[1] === '' ? [0] : m[1].split(/[;:]/).map((p) => Number(p) || 0));
    last = re.lastIndex;
  }
  if (last < text.length) out.push({ text: text.slice(last), style });
  return out;
}

/** CSS for a cell style (colors are theme tokens or literal rgb from 256/truecolor). */
export function cssFor(s: CellStyle): CSSProperties | undefined {
  if (!s.fg && !s.bg && !s.bold && !s.dim && !s.italic && !s.underline && !s.inverse) return undefined;
  const fg = s.inverse ? (s.bg ?? 'var(--term-bg)') : s.fg;
  const bg = s.inverse ? (s.fg ?? 'var(--term-fg)') : s.bg;
  return {
    color: fg,
    background: bg,
    fontWeight: s.bold ? 700 : undefined,
    opacity: s.dim ? 0.7 : undefined,
    fontStyle: s.italic ? 'italic' : undefined,
    textDecoration: s.underline ? 'underline' : undefined,
  };
}

/** Bytes to send for a key press (xterm conventions), or null to let the browser handle it. */
export function keyToData(e: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }, appCursor: boolean): string | null {
  if (e.metaKey) return null;
  const arrow = (c: string) => (appCursor ? `\x1bO${c}` : `\x1b[${c}`);
  switch (e.key) {
    case 'Enter':
      return '\r';
    case 'Backspace':
      return e.ctrlKey || e.altKey ? '\x17' : '\x7f';
    case 'Tab':
      return e.shiftKey ? '\x1b[Z' : '\t';
    case 'Escape':
      return '\x1b';
    case 'ArrowUp':
      return arrow('A');
    case 'ArrowDown':
      return arrow('B');
    case 'ArrowRight':
      return arrow('C');
    case 'ArrowLeft':
      return arrow('D');
    case 'Home':
      return '\x1b[H';
    case 'End':
      return '\x1b[F';
    case 'Delete':
      return '\x1b[3~';
    case 'Insert':
      return '\x1b[2~';
    case 'PageUp':
      return '\x1b[5~';
    case 'PageDown':
      return '\x1b[6~';
    case 'F1':
      return '\x1bOP';
    case 'F2':
      return '\x1bOQ';
    case 'F3':
      return '\x1bOR';
    case 'F4':
      return '\x1bOS';
  }
  if (e.key.length !== 1) return null;
  if (e.ctrlKey && !e.altKey) {
    if (e.shiftKey && (e.key === 'C' || e.key === 'V' || e.key === 'c' || e.key === 'v')) return null; // copy/paste
    const c = e.key.toLowerCase();
    if (c >= 'a' && c <= 'z') return String.fromCharCode(c.charCodeAt(0) - 96);
    if (c === '[') return '\x1b';
    if (c === '\\') return '\x1c';
    if (c === ']') return '\x1d';
    if (c === ' ' || c === '@' || c === '2') return '\x00';
    return null;
  }
  if (e.altKey && !e.ctrlKey) return `\x1b${e.key}`;
  return e.key;
}
