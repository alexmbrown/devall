// Terminal output → React spans: SGR colours/bold/italic/underline become classes (theme-aware,
// see notebooks.css); other escape sequences are dropped; `\r` progress lines keep their last
// state.
import type { CSSProperties, ReactNode } from 'react';

interface Style {
  fg?: number | string;
  bg?: number | string;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
}

// eslint-disable-next-line no-control-regex
const ESCAPES = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[PX^_][\s\S]*?\x1b\\|\x1b[()*+\-./#%].|\x1b[@-Z\\-_]|\x9b[0-?]*[ -/]*[@-~]/g;

/** Removes escape sequences and control characters (keeps \n and \t), resolves \r overwrites. */
export function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex
  return applyCarriageReturns(text.replace(ESCAPES, '')).replace(/[\x00-\x08\x0b-\x1f\x7f]/g, '');
}

/** `\r\n` → `\n`; a line rewritten with `\r` shows its last non-empty state. */
export function applyCarriageReturns(text: string): string {
  if (!text.includes('\r')) return text;
  return text
    .replace(/\r\n/g, '\n')
    .split('\n')
    .map((line) => {
      if (!line.includes('\r')) return line;
      const parts = line.split('\r');
      for (let i = parts.length - 1; i >= 0; i--) if (parts[i]) return parts[i];
      return '';
    })
    .join('\n');
}

const xterm256 = (n: number): string => {
  if (n < 16) return `var(--ansi-${n})`;
  if (n >= 232) {
    const v = 8 + (n - 232) * 10;
    return `rgb(${v} ${v} ${v})`;
  }
  const i = n - 16;
  const c = (x: number) => (x === 0 ? 0 : 55 + x * 40);
  return `rgb(${c(Math.floor(i / 36))} ${c(Math.floor(i / 6) % 6)} ${c(i % 6)})`;
};

function applySgr(params: number[], s: Style): Style {
  const next = { ...s };
  if (!params.length) params = [0];
  for (let i = 0; i < params.length; i++) {
    const p = params[i];
    if (p === 0) {
      for (const k of Object.keys(next) as (keyof Style)[]) delete next[k];
    } else if (p === 1) next.bold = true;
    else if (p === 2) next.dim = true;
    else if (p === 3) next.italic = true;
    else if (p === 4) next.underline = true;
    else if (p === 22) next.bold = next.dim = false;
    else if (p === 23) next.italic = false;
    else if (p === 24) next.underline = false;
    else if (p >= 30 && p <= 37) next.fg = p - 30;
    else if (p >= 90 && p <= 97) next.fg = p - 90 + 8;
    else if (p === 39) delete next.fg;
    else if (p >= 40 && p <= 47) next.bg = p - 40;
    else if (p >= 100 && p <= 107) next.bg = p - 100 + 8;
    else if (p === 49) delete next.bg;
    else if ((p === 38 || p === 48) && params[i + 1] === 5 && params[i + 2] !== undefined) {
      next[p === 38 ? 'fg' : 'bg'] = xterm256(params[i + 2]);
      i += 2;
    } else if ((p === 38 || p === 48) && params[i + 1] === 2 && params[i + 4] !== undefined) {
      next[p === 38 ? 'fg' : 'bg'] = `rgb(${params[i + 2]} ${params[i + 3]} ${params[i + 4]})`;
      i += 4;
    }
  }
  return next;
}

function span(text: string, s: Style, key: number): ReactNode {
  if (!text) return null;
  const cls: string[] = [];
  const style: CSSProperties = {};
  if (typeof s.fg === 'number') cls.push(`ansi-fg-${s.fg}`);
  else if (s.fg) style.color = s.fg;
  if (typeof s.bg === 'number') cls.push(`ansi-bg-${s.bg}`);
  else if (s.bg) style.background = s.bg;
  if (s.bold) cls.push('ansi-bold');
  if (s.dim) cls.push('ansi-dim');
  if (s.italic) cls.push('ansi-italic');
  if (s.underline) cls.push('ansi-underline');
  if (!cls.length && !style.color && !style.background) return text;
  return (
    <span key={key} className={cls.join(' ')} style={style}>
      {text}
    </span>
  );
}

/** Renders raw terminal text with colours. */
export function Ansi({ text }: { text: string }) {
  // eslint-disable-next-line no-control-regex
  const src = applyCarriageReturns(text).replace(/[\x00-\x08\x0b\x0c\x0e-\x1a\x1c-\x1f\x7f]/g, '');
  const out: ReactNode[] = [];
  let style: Style = {};
  let last = 0;
  let key = 0;
  // eslint-disable-next-line no-control-regex
  const re = /\x1b\[([0-9;]*)m|\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[PX^_][\s\S]*?\x1b\\|\x1b[()*+\-./#%].|\x1b[@-Z\\-_]/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src))) {
    out.push(span(src.slice(last, m.index), style, key++));
    if (m[1] !== undefined) {
      const params = m[1] === '' ? [] : m[1].split(';').map((x) => Number(x) || 0);
      style = applySgr(params, style);
    }
    last = re.lastIndex;
  }
  out.push(span(src.slice(last), style, key++));
  return <>{out}</>;
}
