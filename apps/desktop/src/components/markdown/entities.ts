/** HTML entity references in Markdown text (`&amp;`, `&#123;`, `&#x1F600;`). */
export const ENTITY_RE = /&(?:#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{1,31});/g;

/** The named entities people actually write; others are shown as written (and kept). */
const NAMED: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' ',
  ensp: ' ',
  emsp: ' ',
  thinsp: ' ',
  zwj: '‍',
  zwnj: '‌',
  shy: '­',
  copy: '©',
  reg: '®',
  trade: '™',
  hellip: '…',
  mdash: '—',
  ndash: '–',
  lsquo: '‘',
  rsquo: '’',
  ldquo: '“',
  rdquo: '”',
  laquo: '«',
  raquo: '»',
  bull: '•',
  middot: '·',
  times: '×',
  divide: '÷',
  deg: '°',
  plusmn: '±',
  para: '¶',
  sect: '§',
  euro: '€',
  pound: '£',
  yen: '¥',
  cent: '¢',
  larr: '←',
  rarr: '→',
  uarr: '↑',
  darr: '↓',
  harr: '↔',
  lArr: '⇐',
  rArr: '⇒',
  hArr: '⇔',
  le: '≤',
  ge: '≥',
  ne: '≠',
  asymp: '≈',
  infin: '∞',
  check: '✓',
  cross: '✗',
  star: '☆',
  hearts: '♥',
  dagger: '†',
  micro: 'µ',
  frac12: '½',
  frac14: '¼',
  frac34: '¾',
  sup2: '²',
  sup3: '³',
  iexcl: '¡',
  iquest: '¿',
  grave: '`',
  lowbar: '_',
  ast: '*',
  num: '#',
  vert: '|',
  verbar: '|',
  lsqb: '[',
  rsqb: ']',
  lbrack: '[',
  rbrack: ']',
  bsol: '\\',
  tilde: '˜',
};

/** The character(s) an entity reference stands for; the reference itself when unknown. */
export function decodeEntity(ref: string): string {
  const body = ref.slice(1, -1);
  if (body[0] === '#') {
    const code = body[1] === 'x' || body[1] === 'X' ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
    if (!Number.isFinite(code) || code > 0x10ffff) return ref;
    try {
      return code === 0 ? '�' : String.fromCodePoint(code);
    } catch {
      return ref;
    }
  }
  return NAMED[body] ?? ref;
}
