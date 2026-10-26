import { Lexer, type Token, type Tokens } from 'marked';
import type { Mark, Node as PMNode } from 'prosemirror-model';
import { isDevallLink } from '@/lib/links/syntax';
import { M, N, schema } from './schema';
import { decodeEntity, ENTITY_RE } from './entities';
import { inlineKey } from './semantic';

/**
 * Markdown → ProseMirror, keeping enough of the source to write untouched parts back exactly.
 *
 * The file is parsed with `marked` (GFM) — the same parser the read-only `Markdown` component uses,
 * so the editor and the preview agree on what the text means. Line endings are normalised to `\n`
 * for parsing (the original bytes are kept in `MdSource` and written back where nothing changed).
 */

export type Links = Record<string, { href: string; title?: string | null }>;

/** Where an original top-level block sits in the LF-normalised text. */
interface TopInfo {
  start: number;
  srcEnd: number;
  gapEnd: number;
}

/** What the serializer needs to know about the parsed file. */
export class MdSource {
  /** Nodes exactly as parsed (identity: untouched since). */
  readonly originals = new WeakSet<PMNode>();
  /** Original nodes by their `src` (finds a node that was changed and changed back, or recreated). */
  readonly bySrc = new Map<string, PMNode[]>();
  /** Each original node's next sibling (null: it was last). */
  readonly next = new WeakMap<PMNode, PMNode | null>();
  /** `src` of the block that followed a block with that `src`. */
  readonly nextSrc = new Map<string, Set<string | null>>();
  readonly top = new WeakMap<PMNode, TopInfo>();
  /** Nodes found equal to an original (cache). */
  readonly verified = new WeakSet<PMNode>();
  doc: PMNode | null = null;
  /** Leading whitespace before the first block, the whitespace after the last one. */
  lead = '';
  docEnd = '';

  constructor(
    /** The file as read (any line endings, BOM included). */
    readonly text: string,
    /** The LF-normalised text without BOM (what was parsed). */
    readonly lf: string,
    readonly bom: boolean,
    /** Dominant line ending, used for anything newly written. */
    readonly eol: '\n' | '\r\n',
    readonly links: Links,
    /** Hard-break style for new breaks (`\` + newline unless the file uses two spaces). */
    readonly hardBreak: string,
  ) {}

  /** LF-space positions of the `\n`s that were `\r\n` (sorted). */
  private crlfs: number[] | null = null;
  private hasCR: boolean | null = null;
  /** Maps an offset in the LF text to the original text (CRs restored, BOM skipped). */
  orig(offset: number): number {
    const base = this.bom ? 1 : 0;
    if (this.crlfs === null) {
      this.crlfs = [];
      const t = this.text;
      let lf = 0;
      for (let i = base; i < t.length; i++, lf++) {
        if (t[i] === '\r' && t[i + 1] === '\n') {
          this.crlfs.push(lf);
          i++;
        }
      }
    }
    // CRLF pairs whose \n comes before `offset` (a lone \r became a \n of the same length).
    let lo = 0;
    let hi = this.crlfs.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.crlfs[mid] < offset) lo = mid + 1;
      else hi = mid;
    }
    return base + offset + lo;
  }

  /** The original bytes of [from, to) in LF space. */
  slice(from: number, to: number): string {
    this.hasCR ??= this.text.includes('\r');
    if (!this.hasCR) return this.lf.slice(from, to);
    return this.text.slice(this.orig(from), this.orig(to));
  }

  register(node: PMNode, next: PMNode | null) {
    this.originals.add(node);
    this.next.set(node, next);
    const src = node.attrs.src as string | null | undefined;
    if (src == null) return;
    const list = this.bySrc.get(src);
    if (list) list.push(node);
    else this.bySrc.set(src, [node]);
    const ns = (next?.attrs.src as string | null | undefined) ?? null;
    const set = this.nextSrc.get(src);
    if (set) set.add(ns);
    else this.nextSrc.set(src, new Set([ns]));
  }

  /** Whether `node` is (still) exactly what was parsed from its `src`. */
  unchanged(node: PMNode): boolean {
    if (this.originals.has(node) || this.verified.has(node)) return true;
    const src = node.attrs.src as string | null | undefined;
    if (src == null) return false;
    const same = this.bySrc.get(src)?.some((o) => o.eq(node));
    if (same) this.verified.add(node);
    return !!same;
  }

  /** The original node `node` corresponds to by `src` (content may have changed). */
  original(node: PMNode): PMNode | undefined {
    const src = node.attrs.src as string | null | undefined;
    return src == null ? undefined : this.bySrc.get(src)?.find((o) => o.type === node.type);
  }

  /**
   * Whether `b` still follows `a` as it did in the file (their contents may have been edited: a
   * block keeps its `src` while it's edited, so `src` says where it came from).
   */
  adjacent(a: PMNode, b: PMNode | null): boolean {
    if (this.originals.has(a) && this.next.get(a) === b) return true;
    const src = a.attrs.src as string | null | undefined;
    if (src == null) return false;
    const ns = (b?.attrs.src as string | null | undefined) ?? null;
    if (b && ns === null) return false;
    return !!this.nextSrc.get(src)?.has(ns);
  }
}

// ── parsing ────────────────────────────────────────────────────────────────────────────────────

const OPTIONS = { gfm: true, breaks: false, pedantic: false, silent: true } as const;

function lexer(links?: Links): Lexer {
  const lx = new Lexer({ ...OPTIONS } as ConstructorParameters<typeof Lexer>[0]);
  if (links) Object.assign(lx.tokens.links, links);
  return lx;
}

const FRONT_MATTER = /^(---)[ \t]*\n([\s\S]*?\n)??(---|\.\.\.)[ \t]*(?=\n|$)/;

/** Splits a block's raw text into its source and the trailing whitespace (from the last newline on). */
function splitRaw(raw: string): { src: string; tail: string } {
  const m = /\n\s*$/.exec(raw);
  return m ? { src: raw.slice(0, m.index), tail: raw.slice(m.index) } : { src: raw, tail: '' };
}

interface Ctx {
  source: MdSource;
  links: Links;
  /** Top level: record positions. */
  top: boolean;
}

interface Built {
  nodes: PMNode[];
  /** Whitespace before the first block / after the last one. */
  lead: string;
  trail: string;
  /** Positions of top-level blocks (LF offsets), parallel to `nodes`. */
  pos: { start: number; srcEnd: number; gapEnd: number }[];
}

/**
 * Builds the blocks of one container from its tokens. `text` is the container's inner text the
 * tokens were lexed from (positions are found in it); `base` its offset in the file (top level only).
 */
function buildBlocks(tokens: Token[], text: string, ctx: Ctx, base = 0): Built {
  const nodes: PMNode[] = [];
  const pos: Built['pos'] = [];
  const gaps: string[] = [];
  let lead = '';
  let at = 0;
  let known = true;
  for (const t of tokens) {
    if (t.type === 'checkbox') continue;
    let raw = t.raw;
    // A loose task item's first paragraph starts with the checkbox, which isn't in the item text.
    if (t.type === 'paragraph' && (t as Tokens.Paragraph).tokens?.[0]?.type === 'checkbox') raw = raw.slice((t as Tokens.Paragraph).tokens[0].raw.length);
    let start = at;
    if (known && text.startsWith(raw, at)) start = at;
    else if (known) {
      const i = text.indexOf(raw, at);
      if (i >= 0) start = i;
      else known = false;
    }
    if (known) {
      // Anything skipped (shouldn't happen) counts as whitespace before this block.
      const skipped = text.slice(at, start);
      if (nodes.length) gaps[gaps.length - 1] += skipped;
      else lead += skipped;
      at = start + raw.length;
    }
    if (t.type === 'space') {
      if (nodes.length) gaps[gaps.length - 1] += raw;
      else lead += raw;
      continue;
    }
    const { src, tail } = splitRaw(raw);
    const node = buildBlock(t, src, ctx);
    if (!node) {
      if (nodes.length) gaps[gaps.length - 1] += raw;
      else lead += raw;
      continue;
    }
    nodes.push(node);
    gaps.push(tail);
    pos.push({ start: base + start, srcEnd: base + start + src.length, gapEnd: base + start + raw.length });
  }
  // Gaps: whitespace up to the next block; the last one is the container's trailing whitespace.
  const trail = gaps.length ? gaps[gaps.length - 1] : '';
  const out = nodes.map((n, i) => (i < nodes.length - 1 ? n.type.create({ ...n.attrs, gap: gaps[i] }, n.content, n.marks) : n));
  for (let i = 0; i < pos.length - 1; i++) pos[i].gapEnd = pos[i + 1].start;
  return { nodes: out, lead, trail, pos };
}

const emptyParagraph = () => N.paragraph.create();

function buildBlock(t: Token, src: string, ctx: Ctx): PMNode | null {
  const at = { src, gap: null };
  switch (t.type) {
    case 'paragraph':
    case 'text': {
      const tt = t as Tokens.Paragraph | Tokens.Text;
      const inline = 'tokens' in tt && tt.tokens ? buildInline(tt.tokens, [], ctx) : buildInline([{ type: 'text', raw: tt.text, text: tt.text } as Tokens.Text], [], ctx);
      return N.paragraph.create(at, inline);
    }
    case 'heading': {
      const h = t as Tokens.Heading;
      return N.heading.create({ ...at, level: h.depth, setext: !/^ {0,3}#/.test(src) }, buildInline(h.tokens, [], ctx));
    }
    case 'code': {
      const c = t as Tokens.Code;
      if (c.codeBlockStyle === 'indented') return N.codeBlock.create({ ...at, indented: true }, c.text ? schema.text(c.text) : undefined);
      const fence = /^ {0,3}(`{3,}|~{3,})/.exec(src)?.[1] ?? '```';
      return N.codeBlock.create({ ...at, info: (c.lang ?? '').trim(), fence }, c.text ? schema.text(c.text) : undefined);
    }
    case 'hr':
      return N.horizontalRule.create({ ...at, markup: src.trim() });
    case 'blockquote': {
      const q = t as Tokens.Blockquote;
      const inner = buildBlocks(q.tokens, q.text, { ...ctx, top: false });
      return N.blockquote.create(at, inner.nodes.length ? inner.nodes : [emptyParagraph()]);
    }
    case 'list': {
      const l = t as Tokens.List;
      const first = l.items[0]?.raw ?? '';
      const items = l.items.map((it, i) => {
        const { src: isrc, tail } = splitRaw(it.raw);
        const inner = buildBlocks(it.tokens, it.text, { ...ctx, top: false });
        return N.listItem.create({ src: isrc, gap: i < l.items.length - 1 ? tail : null, checked: it.task ? !!it.checked : null }, inner.nodes.length ? inner.nodes : [emptyParagraph()]);
      });
      if (l.ordered) {
        const delim = /^\s*\d{1,9}([.)])/.exec(first)?.[1] ?? '.';
        return N.orderedList.create({ ...at, start: typeof l.start === 'number' ? l.start : 1, delim, tight: !l.loose }, items);
      }
      const bullet = /^\s*([-+*])/.exec(first)?.[1] ?? '-';
      return N.bulletList.create({ ...at, bullet, tight: !l.loose }, items);
    }
    case 'table': {
      const tb = t as Tokens.Table;
      const lines = src.split('\n');
      const rowsSrc = lines.length === tb.rows.length + 2 ? lines : null;
      const row = (cells: Tokens.TableCell[], header: boolean, line: string | null) =>
        N.tableRow.create(
          { src: line },
          cells.map((c, i) => (header ? N.tableHeader : N.tableCell).create({ align: tb.align[i] ?? null }, buildInline(c.tokens, [], ctx))),
        );
      return N.table.create({ ...at, delim: rowsSrc?.[1] ?? null }, [row(tb.header, true, rowsSrc?.[0] ?? null), ...tb.rows.map((r, i) => row(r, false, rowsSrc?.[i + 2] ?? null))]);
    }
    case 'html':
      return N.rawBlock.create({ ...at, kind: 'html' }, src ? schema.text(src) : undefined);
    case 'def':
      return N.rawBlock.create({ ...at, kind: 'def' }, src ? schema.text(src) : undefined);
    default:
      return src ? N.rawBlock.create({ ...at, kind: 'other' }, schema.text(src)) : null;
  }
}

// ── inline ─────────────────────────────────────────────────────────────────────────────────────

/** Line breaks keep the formatting around them (so `**a\nb**` stays one strong run), not escape/entity/code. */
const breakMarks = (marks: readonly Mark[]) => marks.filter((m) => m.type !== M.escape && m.type !== M.entity && m.type !== M.code);

function textNodes(text: string, marks: readonly Mark[], out: PMNode[]) {
  // Soft line breaks become softBreak nodes; entity references get an `entity` mark.
  const lines = text.split('\n');
  lines.forEach((line, li) => {
    if (li > 0) out.push(N.softBreak.create(null, undefined, breakMarks(marks)));
    let last = 0;
    ENTITY_RE.lastIndex = 0;
    for (let m = ENTITY_RE.exec(line); m; m = ENTITY_RE.exec(line)) {
      if (m.index > last) out.push(schema.text(line.slice(last, m.index), marks));
      out.push(schema.text(decodeEntity(m[0]), M.entity.create({ raw: m[0] }).addToSet(marks)));
      last = m.index + m[0].length;
    }
    if (last < line.length) out.push(schema.text(line.slice(last), marks));
  });
}

/** The parts of an inline link's raw source: destination and title as written. */
export function linkParts(raw: string): { dest: string; titleRaw: string | null } | null {
  const m = /\]\(\s*(<[^<>\n]*>|[^\s()<>]*(?:\([^\s()]*\)[^\s()<>]*)*)(?:\s+("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\((?:[^()\\]|\\.)*\)))?\s*\)$/.exec(raw);
  return m ? { dest: m[1], titleRaw: m[2] ?? null } : null;
}

export function buildInline(tokens: Token[], marks: readonly Mark[], ctx: Ctx): PMNode[] {
  const out: PMNode[] = [];
  for (const t of tokens) {
    switch (t.type) {
      case 'text': {
        const tt = t as Tokens.Text;
        if (tt.tokens?.length) out.push(...buildInline(tt.tokens, marks, ctx));
        // The source text: marked decodes numeric entities in `text`; entities are handled here.
        else if (tt.raw || tt.text) textNodes(tt.raw || tt.text, marks, out);
        break;
      }
      case 'escape':
        if (t.text) out.push(schema.text(t.text, M.escape.create().addToSet(marks)));
        break;
      case 'strong':
        out.push(...buildInline((t as Tokens.Strong).tokens, M.strong.create({ marker: t.raw.slice(0, 2) }).addToSet(marks), ctx));
        break;
      case 'em':
        out.push(...buildInline((t as Tokens.Em).tokens, M.em.create({ marker: t.raw[0] }).addToSet(marks), ctx));
        break;
      case 'del':
        out.push(...buildInline((t as Tokens.Del).tokens, M.strike.create({ marker: t.raw.startsWith('~~') ? '~~' : '~' }).addToSet(marks), ctx));
        break;
      case 'codespan': {
        const text = (t as Tokens.Codespan).text;
        if (text) out.push(schema.text(text, M.code.create({ raw: t.raw }).addToSet(marks)));
        break;
      }
      case 'br':
        out.push(N.hardBreak.create({ raw: t.raw }, undefined, breakMarks(marks)));
        break;
      case 'link': {
        const l = t as Tokens.Link;
        if (isDevallLink(l.href) && !marks.some((m) => m.type === M.link)) {
          out.push(N.devallLink.create({ href: l.href, label: l.text, title: l.title ?? null, raw: l.raw }, undefined, marks));
          break;
        }
        let style = 'inline';
        let ref: string | null = null;
        let parts: ReturnType<typeof linkParts> = null;
        if (l.raw.startsWith('<')) style = 'angle';
        else if (!l.raw.startsWith('[')) style = 'bare';
        else {
          parts = linkParts(l.raw);
          if (!parts) {
            const m = /\]\s*\[([^\]]*)\]$/.exec(l.raw);
            if (m) {
              style = m[1] ? 'ref' : 'collapsed';
              ref = m[1] || l.text;
            } else {
              style = 'shortcut';
              ref = l.text;
            }
          }
        }
        const inner = l.tokens?.length ? buildInline(l.tokens, marks, ctx) : [];
        const attrs = { href: l.href, title: l.title ?? null, style, ref, raw: l.raw, dest: parts?.dest ?? null, titleRaw: parts?.titleRaw ?? null };
        // `key`: what the link text means, apart from the formatting around the link.
        if (inner.length) {
          const mark = M.link.create({ ...attrs, key: inlineKey(inner, marks) });
          for (const n of inner) out.push(n.mark(mark.addToSet(n.marks)));
        } else {
          const text = schema.text(l.text || l.href, marks);
          out.push(text.mark(M.link.create({ ...attrs, key: inlineKey([text], marks) }).addToSet(text.marks)));
        }
        break;
      }
      case 'image': {
        const im = t as Tokens.Image;
        out.push(N.image.create({ src: im.href, alt: im.text, title: im.title ?? null, raw: im.raw }, undefined, marks));
        break;
      }
      case 'html':
        out.push(N.htmlInline.create({ raw: t.raw }, undefined, marks));
        break;
      case 'checkbox':
        break;
      default:
        if (t.raw) textNodes(t.raw, marks, out);
    }
  }
  return out;
}

// ── entry points ───────────────────────────────────────────────────────────────────────────────

/** Parses a Markdown file into an editor document and the bookkeeping to write it back. */
export function parseMarkdown(text: string): { doc: PMNode; source: MdSource } {
  const bom = text.charCodeAt(0) === 0xfeff;
  const body = bom ? text.slice(1) : text;
  const lf = body.replace(/\r\n?/g, '\n');
  const crlf = (body.match(/\r\n/g)?.length ?? 0) > (body.match(/(^|[^\r])\n/g)?.length ?? 0);
  const hardBreak = /[^ \n] {2,}\n/.test(lf) && !/\\\n/.test(lf) ? '  \n' : '\\\n';

  const lx = lexer();
  let fm: { node: PMNode; raw: string } | null = null;
  const fmMatch = FRONT_MATTER.exec(lf);
  let rest = lf;
  let base = 0;
  if (fmMatch) {
    const raw = fmMatch[0];
    fm = { node: N.frontMatter.create({ src: raw, open: fmMatch[1], close: fmMatch[3] }, fmMatch[2] ? schema.text(fmMatch[2].replace(/\n$/, '')) : undefined), raw };
    rest = lf.slice(raw.length);
    base = raw.length;
  }
  const tokens = lx.lex(rest);
  const links = { ...(tokens.links as Links) };
  const source = new MdSource(text, lf, bom, crlf ? '\r\n' : '\n', links, hardBreak);
  const ctx: Ctx = { source, links, top: true };
  const built = buildBlocks(tokens, rest, ctx, base);

  let nodes = built.nodes;
  let pos = built.pos;
  if (fm) {
    // The front matter's gap is the whitespace before the first block.
    const fmNode = fm.node.type.create({ ...fm.node.attrs, gap: nodes.length ? built.lead : null }, fm.node.content);
    nodes = [fmNode, ...nodes];
    pos = [{ start: 0, srcEnd: fm.raw.length, gapEnd: nodes.length > 1 ? pos[0].start : lf.length }, ...pos];
    source.docEnd = nodes.length > 1 ? built.trail : built.lead + built.trail;
  } else {
    source.lead = built.lead;
    source.docEnd = built.trail;
  }
  if (!nodes.length) nodes = [emptyParagraph()];
  const doc = N.doc.create(null, nodes);
  source.doc = doc;
  source.originals.add(doc);
  doc.forEach((child, _o, i) => {
    source.register(child, i < doc.childCount - 1 ? doc.child(i + 1) : null);
    if (pos[i]) source.top.set(child, { start: pos[i].start, srcEnd: pos[i].srcEnd, gapEnd: i < doc.childCount - 1 ? pos[i].gapEnd : lf.length });
    registerDeep(child, source);
  });
  return { doc, source };
}

function registerDeep(node: PMNode, source: MdSource) {
  if (node.isTextblock || node.isLeaf) return;
  node.forEach((child, _o, i) => {
    if (child.isInline) return;
    source.register(child, i < node.childCount - 1 ? node.child(i + 1) : null);
    registerDeep(child, source);
  });
}

/** Parses a Markdown fragment (pasted text, a block being verified) with the document's link definitions. */
export function parseFragment(md: string, links: Links = {}): PMNode {
  const lx = lexer(links);
  const tokens = lx.lex(md.replace(/\r\n?/g, '\n'));
  const source = new MdSource(md, md, false, '\n', links, '\\\n');
  const built = buildBlocks(tokens, md, { source, links, top: false });
  return N.doc.create(null, built.nodes.length ? built.nodes : [emptyParagraph()]);
}
