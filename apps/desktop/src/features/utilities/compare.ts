// Pure helpers for the String compare tool (unit-tested in compare.test.ts).

export type TokenKind = 'text' | 'space' | 'tab' | 'cr' | 'lf' | 'nbsp' | 'ws' | 'invisible' | 'bidi' | 'control' | 'bom';

export interface Token {
  kind: TokenKind;
  /** What to render (a visible marker for whitespace and invisible characters). */
  text: string;
  /** Code point + short name for markers. */
  title?: string;
}

const ABBR: Record<number, string> = {
  0x00ad: 'SHY',
  0x034f: 'CGJ',
  0x061c: 'ALM',
  0x180e: 'MVS',
  0x200b: 'ZWSP',
  0x200c: 'ZWNJ',
  0x200d: 'ZWJ',
  0x200e: 'LRM',
  0x200f: 'RLM',
  0x202a: 'LRE',
  0x202b: 'RLE',
  0x202c: 'PDF',
  0x202d: 'LRO',
  0x202e: 'RLO',
  0x2060: 'WJ',
  0x2061: 'FA',
  0x2062: 'IT',
  0x2063: 'IS',
  0x2064: 'IP',
  0x2066: 'LRI',
  0x2067: 'RLI',
  0x2068: 'FSI',
  0x2069: 'PDI',
  0xfeff: 'BOM',
};

export const codePoint = (c: string) => `U+${(c.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, '0')}`;

export function isBidiControl(cp: number) {
  return cp === 0x061c || cp === 0x200e || cp === 0x200f || (cp >= 0x202a && cp <= 0x202e) || (cp >= 0x2066 && cp <= 0x2069);
}

export function isInvisible(cp: number) {
  return (
    [0x00ad, 0x034f, 0x061c, 0x115f, 0x1160, 0x17b4, 0x17b5, 0x180e, 0x3164, 0xfeff, 0xffa0].includes(cp) ||
    (cp >= 0x200b && cp <= 0x200f) ||
    (cp >= 0x2060 && cp <= 0x2064) ||
    (cp >= 0x206a && cp <= 0x206f) ||
    (cp >= 0xe0000 && cp <= 0xe007f)
  );
}

/** Splits `s` into plain text runs and visible markers for whitespace and invisible characters. */
export function visibleTokens(s: string): Token[] {
  const out: Token[] = [];
  let run = '';
  const flush = () => {
    if (run) out.push({ kind: 'text', text: run });
    run = '';
  };
  const push = (t: Token) => {
    flush();
    out.push(t);
  };
  for (const c of s) {
    const cp = c.codePointAt(0) ?? 0;
    if (c === ' ') push({ kind: 'space', text: '·', title: 'U+0020 SPACE' });
    else if (c === '\t') push({ kind: 'tab', text: '→', title: 'U+0009 TAB' });
    else if (c === '\r') push({ kind: 'cr', text: '␍', title: 'U+000D CARRIAGE RETURN' });
    else if (c === '\n') push({ kind: 'lf', text: '↵\n', title: 'U+000A LINE FEED' });
    else if (cp === 0xa0 || cp === 0x202f) push({ kind: 'nbsp', text: '⍽', title: `${codePoint(c)} ${cp === 0xa0 ? 'NO-BREAK SPACE' : 'NARROW NO-BREAK SPACE'}` });
    else if (cp === 0xfeff) push({ kind: 'bom', text: '⟨BOM⟩', title: 'U+FEFF BYTE ORDER MARK / ZERO WIDTH NO-BREAK SPACE' });
    else if (isBidiControl(cp)) push({ kind: 'bidi', text: `⟨${ABBR[cp] ?? codePoint(c)}⟩`, title: `${codePoint(c)} bidirectional control` });
    else if (isInvisible(cp)) push({ kind: 'invisible', text: `⟨${ABBR[cp] ?? codePoint(c)}⟩`, title: `${codePoint(c)} invisible` });
    else if (/\s/u.test(c)) push({ kind: 'ws', text: '␣', title: `${codePoint(c)} whitespace` });
    else if (/\p{Cc}/u.test(c)) push({ kind: 'control', text: `⟨${codePoint(c)}⟩`, title: `${codePoint(c)} control character` });
    else run += c;
  }
  flush();
  return out;
}

/** Textareas turn CR into LF; the inputs show CR as ␍ (U+240D) so pasted CRLF survives editing. */
export const CR_MARK = '␍';
export const toDisplay = (raw: string) => raw.replace(/\r/g, CR_MARK);
export const fromDisplay = (shown: string) => shown.replace(/␍/g, '\r');

/** Decodes backslash escapes (`\n \r \t \0 \\ \" \' \xNN \uXXXX \u{X…}`). */
export function decodeEscapes(s: string): { value: string; error?: string } {
  let out = '';
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c !== '\\' || i === s.length - 1) {
      out += c;
      continue;
    }
    const n = s[++i];
    const simple: Record<string, string> = { n: '\n', r: '\r', t: '\t', '0': '\0', b: '\b', f: '\f', v: '\v', '\\': '\\', '"': '"', "'": "'" };
    if (n in simple) {
      out += simple[n];
    } else if (n === 'x') {
      const hex = s.slice(i + 1, i + 3);
      if (!/^[0-9a-fA-F]{2}$/.test(hex)) return { value: out, error: `\\x needs two hex digits at ${i}` };
      out += String.fromCharCode(parseInt(hex, 16));
      i += 2;
    } else if (n === 'u' && s[i + 1] === '{') {
      const end = s.indexOf('}', i + 2);
      const hex = end > 0 ? s.slice(i + 2, end) : '';
      const cp = parseInt(hex, 16);
      if (!/^[0-9a-fA-F]{1,6}$/.test(hex) || cp > 0x10ffff) return { value: out, error: `invalid \\u{…} escape at ${i}` };
      out += String.fromCodePoint(cp);
      i = end;
    } else if (n === 'u') {
      const hex = s.slice(i + 1, i + 5);
      if (!/^[0-9a-fA-F]{4}$/.test(hex)) return { value: out, error: `\\u needs four hex digits at ${i}` };
      out += String.fromCharCode(parseInt(hex, 16));
      i += 4;
    } else {
      out += `\\${n}`;
    }
  }
  return { value: out };
}

export type Range = [number, number];

/** UTF-16 ranges of A (deleted) and B (inserted) text in a character diff, for highlighting. */
export function diffRanges(segments: { tag: 'equal' | 'delete' | 'insert'; value: string }[]): { a: Range[]; b: Range[] } {
  const a: Range[] = [];
  const b: Range[] = [];
  let ia = 0;
  let ib = 0;
  for (const s of segments) {
    const n = s.value.length;
    if (s.tag === 'equal') {
      ia += n;
      ib += n;
    } else if (s.tag === 'delete') {
      a.push([ia, ia + n]);
      ia += n;
    } else {
      b.push([ib, ib + n]);
      ib += n;
    }
  }
  return { a, b };
}

/** Short label for line-ending styles. */
export function lineEndingLabel(v: string) {
  return v === 'none' ? '—' : v;
}
