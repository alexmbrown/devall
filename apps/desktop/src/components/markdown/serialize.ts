import type { Mark, Node as PMNode } from 'prosemirror-model';
import { decodeEntity } from './entities';
import { parseFragment, type Links, type MdSource } from './parse';
import { DELIMITER_MARKS, N } from './schema';
import { inlineKey, sameMeaning } from './semantic';

/**
 * ProseMirror → Markdown.
 *
 * Source-preserving: a block that still equals what was parsed is written back from its `src`
 * (top-level blocks from the file's original bytes, line endings included), so only blocks the
 * user changed are re-written — and those keep their style where it can be known (list markers,
 * emphasis characters, heading style, code fences, link reference style, escapes, entities).
 *
 * A re-written block is parsed again and compared with the editor's block (`sameMeaning`); if it
 * reads back differently (an emphasis that can't close, a character that needs escaping), it's
 * written again with plainer choices (`*` markers, then escaping every special character).
 */

interface Pass {
  /** Keep `_`/`__`/`~` markers as written, or use `*`/`**`/`~~`. */
  markers: 'keep' | 'star';
  /** Escape only where needed, or every Markdown-special character. */
  escape: 'min' | 'all';
}

const PASSES: Pass[] = [
  { markers: 'keep', escape: 'min' },
  { markers: 'star', escape: 'min' },
  { markers: 'star', escape: 'all' },
];

const isWs = (c: string | undefined) => c === undefined || c === '' || /\s/.test(c);
const isAlnum = (c: string | undefined) => !!c && /[\p{L}\p{N}]/u.test(c);
const isPunct = (c: string | undefined) => !!c && /[!-/:-@[-`{-~]/.test(c);

const kids = (n: PMNode) => {
  const out: PMNode[] = [];
  n.forEach((c) => out.push(c));
  return out;
};

/** The text a code span's source stands for (CommonMark: newlines → spaces, one padding space stripped). */
function codeSpanText(raw: string): string {
  const ticks = /^`+/.exec(raw)?.[0].length ?? 0;
  let t = raw.slice(ticks, raw.length - ticks).replace(/\n/g, ' ');
  if (t.length > 2 && t.startsWith(' ') && t.endsWith(' ') && t.trim() !== '') t = t.slice(1, -1);
  return t;
}

function codeSpan(text: string): string {
  const longest = Math.max(0, ...(text.match(/`+/g) ?? []).map((r) => r.length));
  const ticks = '`'.repeat(longest + 1);
  const pad = text.startsWith('`') || text.endsWith('`') || (text.length > 1 && text.startsWith(' ') && text.endsWith(' ') && text.trim() !== '') ? ' ' : '';
  return ticks + pad + text + pad + ticks;
}

/** A link destination: as is, or in angle brackets when it has spaces or unbalanced parentheses. */
export function formatDest(href: string): string {
  let depth = 0;
  let balanced = true;
  for (const c of href) {
    if (c === '(') depth++;
    else if (c === ')' && --depth < 0) balanced = false;
  }
  if (!href || /[\s<>]/.test(href) || !balanced || depth !== 0) return `<${href.replace(/[<>]/g, (c) => `\\${c}`)}>`;
  return href;
}

const formatTitle = (t: string) => ` "${t.replace(/["\\]/g, (c) => `\\${c}`)}"`;

/** Whether a link reference label refers to a definition (case-insensitive, whitespace-collapsed). */
const refKey = (s: string) => s.trim().replace(/\s+/g, ' ').toLowerCase();

class Writer {
  /** Fresh output per node (nodes are immutable). */
  private cache = new WeakMap<PMNode, string>();
  private defs: Set<string>;

  constructor(
    readonly source: MdSource | null,
    readonly links: Links,
    readonly hardBreak: string,
  ) {
    this.defs = new Set(Object.keys(links).map(refKey));
  }

  hasDef(label: string) {
    return this.defs.has(refKey(label));
  }

  unchanged(n: PMNode): boolean {
    return !!this.source && n.attrs.src != null && this.source.unchanged(n);
  }

  /** The separator between two sibling blocks: the original one while they're still neighbours. */
  sep(a: PMNode, b: PMNode, dflt: string): string {
    const g = a.attrs.gap as string | null;
    if (g != null && this.source) {
      if (this.source.adjacent(a, b)) return g;
      if ((g.match(/\n/g)?.length ?? 0) >= 2) return g;
    }
    return dflt;
  }

  /** Markdown for one block (LF line endings, no trailing newline). */
  block(n: PMNode): string {
    if (this.unchanged(n)) return n.attrs.src as string;
    const cached = this.cache.get(n);
    if (cached !== undefined) return cached;
    let out = '';
    if (n.isTextblock && n.type !== N.codeBlock && n.type !== N.rawBlock && n.type !== N.frontMatter) out = this.verified(n);
    else if (n.type === N.table) out = this.verified(n);
    else out = this.fresh(n, PASSES[0]);
    this.cache.set(n, out);
    return out;
  }

  /** Writes `n` with the first pass that reads back as the same thing. */
  private verified(n: PMNode): string {
    let out = '';
    for (const pass of PASSES) {
      out = this.fresh(n, pass);
      const back = parseFragment(out, this.links);
      if (back.childCount === 1 && sameMeaning(back.firstChild!, n)) return out;
      // An empty block writes nothing.
      if (!out.trim() && !n.textContent.trim()) return out;
    }
    return out;
  }

  /** Blocks of a container, joined with their separators. */
  children(parent: PMNode, dflt: (a: PMNode, b: PMNode) => string): string {
    let list = kids(parent);
    // Empty paragraphs have no Markdown (except as the only content of an item/quote).
    if (list.length > 1) list = list.filter((c) => !(c.type === N.paragraph && c.childCount === 0 && c.attrs.src == null));
    let out = '';
    list.forEach((c, i) => {
      out += this.block(c);
      if (i < list.length - 1) out += this.sep(c, list[i + 1], dflt(c, list[i + 1]));
    });
    return out;
  }

  fresh(n: PMNode, pass: Pass): string {
    const src = n.attrs.src as string | null;
    switch (n.type.name) {
      case 'paragraph':
        return new Inline(this, pass, 'block').run(n);
      case 'heading': {
        const content = new Inline(this, pass, 'heading').run(n);
        const level = n.attrs.level as number;
        if (n.attrs.setext && level <= 2 && content) {
          const under = src ? /\n( {0,3}(=+|-+)[ \t]*)$/.exec(src)?.[1] : undefined;
          const ch = level === 1 ? '=' : '-';
          return `${content}\n${under && under.trim()[0] === ch ? under : ch.repeat(Math.max(3, Math.min(content.length, 80)))}`;
        }
        const closing = src && /^ {0,3}#/.test(src) ? (/[ \t]+#+[ \t]*$/.exec(src)?.[0] ?? '') : '';
        return content ? `${'#'.repeat(level)} ${content}${closing}` : '#'.repeat(level);
      }
      case 'codeBlock': {
        const text = n.textContent;
        if (n.attrs.indented && !n.attrs.info) return text.split('\n').map((l) => (l ? `    ${l}` : '')).join('\n');
        let fence = (n.attrs.fence as string) || '```';
        // A fence inside the code must be shorter than ours.
        const ch = fence[0];
        const inner = text.match(new RegExp(`^ {0,3}${ch === '`' ? '`' : '~'}{3,}`, 'gm')) ?? [];
        const longest = Math.max(0, ...inner.map((f) => f.trim().length));
        if (longest >= fence.length) fence = ch.repeat(longest + 1);
        const info = n.attrs.info ? `${n.attrs.info}` : '';
        return `${fence}${info}\n${text ? `${text}\n` : ''}${fence}`;
      }
      case 'horizontalRule':
        return (n.attrs.markup as string) || '---';
      case 'blockquote': {
        const inner = this.children(n, () => '\n\n');
        const prefix = src && /^ {0,3}>(?! )/.test(src) && !/^ {0,3}> /m.test(src) ? '>' : '> ';
        return inner
          .split('\n')
          .map((l) => (l ? prefix + l : '>'))
          .join('\n');
      }
      case 'bulletList':
      case 'orderedList':
        return this.list(n);
      case 'listItem':
        return this.item(n, null, null);
      case 'table':
        return this.table(n, pass);
      case 'frontMatter': {
        const text = n.textContent;
        return `${n.attrs.open}\n${text ? `${text}\n` : ''}${n.attrs.close}`;
      }
      default:
        return n.textContent;
    }
  }

  private list(n: PMNode): string {
    const items = kids(n);
    const ordered = n.type === N.orderedList;
    const tight = n.attrs.tight as boolean;
    // Numbering: every item the same number (`1.` `1.` `1.`) when the file did that, else counting up.
    let same = false;
    if (ordered) {
      const nums = items.map((it) => (it.attrs.src != null ? /^ {0,3}(\d{1,9})[.)]/.exec(it.attrs.src)?.[1] : undefined)).filter((x): x is string => !!x);
      same = nums.length > 1 && nums.every((x) => x === nums[0]);
    }
    const start = (n.attrs.start as number) ?? 1;
    let out = '';
    items.forEach((it, i) => {
      out += this.item(it, n, ordered ? (same ? start : start + i) : null);
      if (i < items.length - 1) {
        const g = it.attrs.gap as string | null;
        out += g != null && this.source?.adjacent(it, items[i + 1]) ? g : tight ? '\n' : '\n\n';
      }
    });
    return out;
  }

  /** A list item, its marker adjusted to the list it's in (`num`: its number in an ordered list). */
  private item(it: PMNode, list: PMNode | null, num: number | null): string {
    const src = it.attrs.src as string | null;
    const bullet = (list?.attrs.bullet as string | undefined) ?? '-';
    const delim = (list?.attrs.delim as string | undefined) ?? '.';
    const marker = num === null ? bullet : `${num}${delim}`;
    const fix = (s: string) => s.replace(/^( {0,3})([-+*]|\d{1,9}[.)])/, (_m, lead: string) => lead + marker);
    if (src != null && this.source) {
      if (this.source.unchanged(it)) return fix(src);
      // Only the checkbox changed: flip it in the source, nothing else moves.
      const orig = this.source.original(it);
      if (orig && orig.attrs.checked !== null && it.attrs.checked !== null && orig.content.eq(it.content)) {
        return fix(src.replace(/^( {0,3}(?:[-+*]|\d{1,9}[.)])[ \t]+)\[[ xX]\]/, (_m, head: string) => `${head}[${it.attrs.checked ? 'x' : ' '}]`));
      }
    }
    const m = src ? /^( {0,3})(?:[-+*]|\d{1,9}[.)])([ \t]+|$)/.exec(src) : null;
    const lead = m?.[1] ?? '';
    const space = m?.[2] && !m[2].includes('\t') && m[2].length <= 4 ? m[2] : ' ';
    const head = `${lead}${marker}${space}`;
    const box = it.attrs.checked === null ? '' : `[${it.attrs.checked ? (src && /\[X\]/.test(src.split('\n')[0]) ? 'X' : 'x') : ' '}]`;
    const tight = (list?.attrs.tight as boolean | undefined) ?? true;
    const content = this.children(it, (a, b) => (tight && a.type === N.paragraph && (b.type === N.bulletList || b.type === N.orderedList) ? '\n' : '\n\n'));
    if (!content) return `${lead}${marker}${box ? ` ${box}` : ''}`;
    const indent = ' '.repeat(head.length);
    const lines = content.split('\n');
    return [head + (box ? `${box} ` : '') + lines[0], ...lines.slice(1).map((l) => (l ? indent + l : ''))].join('\n');
  }

  private table(n: PMNode, pass: Pass): string {
    const rows = kids(n);
    const cols = Math.max(1, ...rows.map((r) => r.childCount));
    const aligns: (string | null)[] = [];
    rows[0]?.forEach((c) => aligns.push(c.attrs.align as string | null));
    while (aligns.length < cols) aligns.push(null);
    const lines = rows.map((r) => {
      if (r.attrs.src != null && this.source?.unchanged(r)) return r.attrs.src as string;
      const cells = kids(r).map((c) => new Inline(this, pass, 'table').run(c));
      while (cells.length < cols) cells.push('');
      return `|${cells.map((c) => (c ? ` ${c} ` : ' ')).join('|')}|`;
    });
    // The delimiter row as written while the columns and alignments still match.
    let delim = n.attrs.delim as string | null;
    if (delim) {
      const cells = delim.trim().replace(/^\|/, '').replace(/\|$/, '').split('|');
      const ok =
        cells.length === cols &&
        cells.every((c, i) => {
          const m = /^\s*(:?)-+(:?)\s*$/.exec(c);
          if (!m) return false;
          const a = m[1] && m[2] ? 'center' : m[1] ? 'left' : m[2] ? 'right' : null;
          return a === aligns[i];
        });
      if (!ok) delim = null;
    }
    delim ??= `| ${aligns.map((a) => (a === 'left' ? ':--' : a === 'center' ? ':-:' : a === 'right' ? '--:' : '---')).join(' | ')} |`;
    return [lines[0], delim, ...lines.slice(1)].join('\n');
  }
}

type InlineCtx = 'block' | 'heading' | 'table';

/** Writes the inline content of one textblock. */
class Inline {
  out = '';
  /** Nothing but whitespace written on the current line yet. */
  private lineStart = true;
  private inLink = 0;
  private plain = '';
  private pos = 0;
  private tildes = 0;

  constructor(
    readonly w: Writer,
    readonly pass: Pass,
    readonly ctx: InlineCtx,
  ) {}

  run(parent: PMNode): string {
    this.plain = parent.textContent;
    this.tildes = this.plain.split('~').length - 1;
    this.nodes(kids(parent), []);
    return this.ctx === 'block' ? this.out : this.out.replace(/\n/g, ' ');
  }

  private emit(s: string) {
    if (!s) return;
    this.out += s;
    const nl = s.lastIndexOf('\n');
    const tail = nl >= 0 ? s.slice(nl + 1) : s;
    if (nl >= 0) this.lineStart = !tail.trim();
    else if (tail.trim()) this.lineStart = false;
  }

  private last(): string | undefined {
    return this.out[this.out.length - 1];
  }

  private nodes(list: PMNode[], outer: Mark[]) {
    let i = 0;
    while (i < list.length) {
      const n = list[i];
      // The mark to open next: the one running longest (emphasis before links on a tie, code always
      // innermost — a code span can't contain formatting).
      let cands = n.marks.filter((x) => DELIMITER_MARKS.has(x.type.name) && !x.isInSet(outer));
      if (cands.length > 1) cands = cands.filter((x) => x.type.name !== 'code');
      if (!cands.length) {
        this.leaf(n, list[i + 1]);
        i++;
        continue;
      }
      let best = cands[0];
      let bestEnd = -1;
      for (const c of cands) {
        let j = i + 1;
        while (j < list.length && c.isInSet(list[j].marks)) j++;
        if (j > bestEnd || (j === bestEnd && best.type.name === 'link' && c.type.name !== 'link')) {
          best = c;
          bestEnd = j;
        }
      }
      this.wrap(best, list.slice(i, bestEnd), [...outer, best], list[bestEnd]);
      i = bestEnd;
    }
  }

  /** Writes a nested run into a separate buffer (to wrap it: an opening delimiter goes first, so no line-start rules). */
  private sub(list: PMNode[], outer: Mark[]): string {
    const saved = this.out;
    const lineStart = this.lineStart;
    this.out = '';
    this.lineStart = false;
    this.nodes(list, outer);
    const inner = this.out;
    this.out = saved;
    this.lineStart = lineStart;
    return inner;
  }

  private wrap(m: Mark, group: PMNode[], outer: Mark[], after: PMNode | undefined) {
    const name = m.type.name;
    if (name === 'code') {
      const text = group.map((g) => (g.isText ? g.text : g.type === N.softBreak ? ' ' : g.textContent)).join('');
      this.pos += text.length;
      // In a table cell a pipe ends the cell even inside a code span: GFM wants it escaped there too.
      // (The parser saw the cell after GFM removed those backslashes, so `raw` has bare pipes.)
      const raw = m.attrs.raw as string | null;
      const span = raw && codeSpanText(raw) === text ? raw : codeSpan(text);
      this.emit(this.ctx === 'table' ? span.replace(/\|/g, '\\|') : span);
      return;
    }
    if (name === 'link') {
      const href = m.attrs.href as string;
      if (m.attrs.raw && m.attrs.key === inlineKey(group, outer)) {
        this.pos += group.reduce((s, g) => s + g.textContent.length, 0);
        this.emit(m.attrs.raw);
        return;
      }
      const text = group.map((g) => g.textContent).join('');
      const plainOnly = group.every((g) => g.isText && g.marks.length === 1);
      if ((m.attrs.style === 'angle' || m.attrs.style === 'bare') && plainOnly && (text === href || `mailto:${text}` === href) && !/[\s<>]/.test(text)) {
        this.pos += text.length;
        this.emit(m.attrs.style === 'bare' && /^(https?:\/\/|www\.)/i.test(text) ? text : `<${text}>`);
        return;
      }
      this.inLink++;
      const inner = this.sub(group, outer);
      this.inLink--;
      const ref = m.attrs.ref as string | null;
      if (ref && (m.attrs.style === 'ref' || m.attrs.style === 'collapsed' || m.attrs.style === 'shortcut') && this.w.hasDef(ref)) {
        if (m.attrs.style !== 'ref' && refKey(text) === refKey(ref)) this.emit(m.attrs.style === 'collapsed' ? `[${inner}][]` : `[${inner}]`);
        else this.emit(`[${inner}][${ref}]`);
        return;
      }
      const dest = m.attrs.dest != null && (m.attrs.dest === href || m.attrs.dest === `<${href}>`) ? (m.attrs.dest as string) : formatDest(href);
      const title = m.attrs.title ? (m.attrs.titleRaw ? ` ${m.attrs.titleRaw}` : formatTitle(m.attrs.title)) : '';
      this.emit(`[${inner}](${dest}${title})`);
      return;
    }
    // strong / em / strike
    const star = name === 'strong' ? '**' : name === 'em' ? '*' : '~~';
    let marker = this.pass.markers === 'keep' ? ((m.attrs.marker as string) ?? star) : star;
    if (marker[0] === '_') {
      const before = this.last();
      const next = after ? (after.isText ? after.text![0] : after.textContent[0]) : undefined;
      if (isAlnum(before) || isAlnum(next)) marker = star;
    }
    const inner = this.sub(group, outer);
    const lead = /^\s*/.exec(inner)![0];
    const trail = inner.length > lead.length ? /\s*$/.exec(inner)![0] : '';
    const core = inner.slice(lead.length, inner.length - trail.length);
    if (!core) {
      this.emit(inner);
      return;
    }
    this.emit(lead);
    this.emit(marker + core + marker);
    this.emit(trail);
  }

  private leaf(n: PMNode, after: PMNode | undefined) {
    switch (n.type.name) {
      case 'text':
        this.text(n, after);
        return;
      case 'softBreak':
        // Trailing spaces before a line break would make it a hard break.
        this.out = this.out.replace(/[ \t]+$/, '');
        this.pos += 1;
        this.emit('\n');
        return;
      case 'hardBreak':
        this.pos += 1;
        if (this.ctx !== 'block') this.emit(' ');
        else this.emit((n.attrs.raw as string) || this.w.hardBreak);
        this.lineStart = true;
        return;
      case 'image': {
        this.pos += n.textContent.length;
        const { src, alt, title, raw } = n.attrs as { src: string; alt: string; title: string | null; raw: string | null };
        this.emit(raw ?? `![${alt.replace(/[[\]]/g, (c) => `\\${c}`)}](${formatDest(src)}${title ? formatTitle(title) : ''})`);
        return;
      }
      case 'devallLink': {
        this.pos += n.textContent.length;
        const { href, label, title, raw } = n.attrs as { href: string; label: string; title: string | null; raw: string | null };
        this.emit(raw ?? `[${label}](${formatDest(href)}${title ? formatTitle(title) : ''})`);
        return;
      }
      case 'htmlInline':
        this.pos += n.textContent.length;
        this.emit(n.attrs.raw as string);
        return;
      default:
        this.pos += n.textContent.length;
        this.emit(this.escape(n.textContent, undefined));
    }
  }

  private text(n: PMNode, after: PMNode | undefined) {
    const text = n.text ?? '';
    const entity = n.marks.find((m) => m.type.name === 'entity');
    const next = after ? (after.isText ? after.text![0] : after.type === N.softBreak ? '\n' : after.textContent[0]) : undefined;
    if (entity && decodeEntity(entity.attrs.raw) === text) {
      this.pos += text.length;
      this.emit(entity.attrs.raw as string);
      return;
    }
    // Characters written escaped (`\*`, `\#`; adjacent ones share a text node) stay escaped.
    if (n.marks.some((m) => m.type.name === 'escape')) {
      this.pos += text.length;
      this.emit([...text].map((c) => (isPunct(c) ? `\\${c}` : c)).join(''));
      return;
    }
    this.emit(this.escape(text, next));
    this.pos += text.length;
  }

  /** Escapes what would otherwise be read as Markdown (`next`: the character after `text`). */
  private escape(text: string, next: string | undefined): string {
    const all = this.pass.escape === 'all';
    const table = this.ctx === 'table';
    let res = '';
    let lineStart = this.lineStart && this.ctx === 'block';
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      const prev = i > 0 ? text[i - 1] : this.last();
      const nx = i < text.length - 1 ? text[i + 1] : next;
      if (lineStart && c !== ' ' && c !== '\t') {
        lineStart = false;
        const rest = text.slice(i);
        const ord = /^(\d{1,9})([.)])(?=[ \t]|$)/.exec(rest);
        if (ord) {
          res += `${ord[1]}\\${ord[2]}`;
          i += ord[0].length - 1;
          continue;
        }
        if (/^(#{1,6}(?=[ \t]|$)|>|[-+*](?=[ \t]|$)|=+[ \t]*$|-+[ \t]*$|(?:[-*_][ \t]*){3,}$|~~~|```)/.test(rest)) {
          res += `\\${c}`;
          continue;
        }
      }
      if (c !== ' ' && c !== '\t') lineStart = false;
      let esc = false;
      switch (c) {
        case '\\':
          esc = all || isPunct(nx) || nx === '\n' || nx === undefined;
          break;
        case '*':
          esc = all || !(isWs(prev) && isWs(nx));
          break;
        case '_':
          esc = all || !((isAlnum(prev) && isAlnum(nx)) || (isWs(prev) && isWs(nx)));
          break;
        case '~':
          esc = all || prev === '~' || nx === '~' || this.tildes >= 2;
          break;
        case '`':
          esc = true;
          break;
        case '[': {
          if (all || this.inLink) {
            esc = true;
            break;
          }
          // Only a bracket that could start a link or a reference.
          const at = this.pos + i;
          const close = this.plain.indexOf(']', at + 1);
          if (close < 0) break;
          const after2 = this.plain[close + 1];
          esc = after2 === '(' || after2 === '[' || this.w.hasDef(this.plain.slice(at + 1, close));
          break;
        }
        case ']':
          esc = all || this.inLink > 0;
          break;
        case '<':
          esc = all || /[A-Za-z/!?]/.test(nx ?? '');
          break;
        case '>':
          esc = all;
          break;
        case '&':
          esc = /^&(?:#\d{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{1,31});/.test(text.slice(i));
          break;
        case '|':
          esc = all || table;
          break;
        case '#':
        case '!':
        case '+':
          esc = all && (c !== '#' || this.ctx !== 'heading' || i > 0);
          break;
      }
      res += esc ? `\\${c}` : c;
    }
    return res;
  }
}

export interface SerializeOptions {
  /** The parsed file this document came from (keeps untouched blocks byte-for-byte). */
  source?: MdSource | null;
}

/** The whole document as Markdown — the original text when nothing changed. */
export function serializeMarkdown(doc: PMNode, source?: MdSource | null): string {
  if (source && (source.originals.has(doc) || (source.doc && source.doc.eq(doc)))) return source.text;
  const w = new Writer(source ?? null, source?.links ?? {}, source?.hardBreak ?? '\\\n');
  const eol = source?.eol ?? '\n';
  const conv = (s: string) => (eol === '\n' ? s : s.replace(/\n/g, eol));
  let list = kids(doc);
  if (list.length > 1) list = list.filter((c) => !(c.type === N.paragraph && c.childCount === 0 && c.attrs.src == null));
  let out = (source?.bom ? '\ufeff' : '') + conv(source?.lead ?? '');
  list.forEach((n, i) => {
    const next = list[i + 1] ?? null;
    const top = source?.originals.has(n) ? source.top.get(n) : undefined;
    // Untouched, and still followed by what followed it: the file's own bytes, separator included.
    if (top && source!.adjacent(n, next) && (next || source!.next.get(n) === null)) {
      out += source!.slice(top.start, top.gapEnd);
      return;
    }
    out += top ? source!.slice(top.start, top.srcEnd) : conv(w.block(n));
    out += conv(next ? w.sep(n, next, '\n\n') : (source?.docEnd ?? ''));
  });
  return out;
}

/** Markdown for a slice of blocks (copy to the clipboard), untouched blocks as written. */
export function serializeBlocks(nodes: readonly PMNode[], source?: MdSource | null): string {
  const w = new Writer(source ?? null, source?.links ?? {}, source?.hardBreak ?? '\\\n');
  return nodes
    .filter((n) => !(n.type === N.paragraph && n.childCount === 0))
    .map((n) => w.block(n))
    .join('\n\n');
}
