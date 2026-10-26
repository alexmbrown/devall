import { Schema, type DOMOutputSpec, type Mark, type Node as PMNode, type NodeSpec } from 'prosemirror-model';

/**
 * The ProseMirror schema of the Markdown editor: exactly the Markdown (GFM) constructs, so a
 * document maps back to Markdown one to one.
 *
 * Every block carries two bookkeeping attributes filled by the parser (`codec.ts`) and never
 * rendered: `src` — the block's Markdown source as it was in the file (relative to its container's
 * indentation) — and `gap` — the whitespace that followed it. The serializer writes `src` back
 * verbatim while the block still equals what was parsed, which is what keeps unedited parts of a
 * file byte-for-byte identical.
 */

const SRC = { src: { default: null }, gap: { default: null } };

const block = (spec: NodeSpec): NodeSpec => ({ group: 'block', ...spec, attrs: { ...SRC, ...spec.attrs } });

const alignStyle = (align: string | null) => (align ? `text-align: ${align}` : null);

function cellAttrs(dom: HTMLElement) {
  const a = dom.getAttribute('align') || dom.style.textAlign || null;
  return { align: a === 'left' || a === 'center' || a === 'right' ? a : null };
}

const cell = (tag: 'td' | 'th', role: 'cell' | 'header_cell'): NodeSpec => ({
  content: 'inline*',
  attrs: { colspan: { default: 1 }, rowspan: { default: 1 }, colwidth: { default: null }, align: { default: null } },
  tableRole: role,
  isolating: true,
  parseDOM: [{ tag, getAttrs: (dom) => cellAttrs(dom as HTMLElement) }],
  toDOM: (n) => [tag, { style: alignStyle(n.attrs.align) }, 0],
});

export const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    paragraph: block({ content: 'inline*', parseDOM: [{ tag: 'p' }], toDOM: () => ['p', 0] }),
    heading: block({
      content: 'inline*',
      defining: true,
      attrs: { level: { default: 1 }, setext: { default: false } },
      parseDOM: [1, 2, 3, 4, 5, 6].map((level) => ({ tag: `h${level}`, attrs: { level } })),
      toDOM: (n) => [`h${n.attrs.level}`, 0],
    }),
    blockquote: block({ content: 'block+', defining: true, parseDOM: [{ tag: 'blockquote' }], toDOM: () => ['blockquote', 0] }),
    codeBlock: block({
      content: 'text*',
      marks: '',
      code: true,
      defining: true,
      // `info`: the whole info string (```js title=x → `js title=x`); `fence`: ``` or ~~~ (any length).
      attrs: { info: { default: '' }, fence: { default: '```' }, indented: { default: false } },
      parseDOM: [
        {
          tag: 'pre',
          preserveWhitespace: 'full',
          getAttrs: (dom) => {
            const el = dom as HTMLElement;
            const cls = `${el.className} ${el.querySelector('code')?.className ?? ''}`;
            const m = /(?:^|\s)(?:language|lang)-([\w+#.-]+)/.exec(cls);
            return { info: m?.[1] ?? el.getAttribute('data-language') ?? '' };
          },
        },
      ],
      toDOM: (n) => ['pre', { 'data-language': n.attrs.info || null }, ['code', 0]],
    }),
    horizontalRule: block({ attrs: { markup: { default: '---' } }, parseDOM: [{ tag: 'hr' }], toDOM: () => ['div', { class: 'md-hr' }, ['hr']] }),
    bulletList: block({
      content: 'listItem+',
      attrs: { bullet: { default: '-' }, tight: { default: true } },
      parseDOM: [{ tag: 'ul', getAttrs: (dom) => ({ tight: !(dom as HTMLElement).querySelector(':scope > li > p + *') }) }],
      toDOM: (n) => ['ul', { class: n.attrs.tight ? 'md-tight' : null }, 0],
    }),
    orderedList: block({
      content: 'listItem+',
      attrs: { start: { default: 1 }, delim: { default: '.' }, tight: { default: true } },
      parseDOM: [{ tag: 'ol', getAttrs: (dom) => ({ start: Number((dom as HTMLElement).getAttribute('start') ?? 1) || 1 }) }],
      toDOM: (n) => ['ol', { start: n.attrs.start === 1 ? null : n.attrs.start, class: n.attrs.tight ? 'md-tight' : null }, 0],
    }),
    listItem: {
      content: 'block+',
      defining: true,
      // `checked`: null for an ordinary item, true/false for a task item (`- [x]` / `- [ ]`).
      attrs: { ...SRC, checked: { default: null } },
      parseDOM: [
        {
          tag: 'li',
          getAttrs: (dom) => {
            const el = dom as HTMLElement;
            const box = el.querySelector(':scope > input[type=checkbox], :scope > p > input[type=checkbox]') as HTMLInputElement | null;
            const data = el.getAttribute('data-checked');
            return { checked: box ? box.checked : data === 'true' ? true : data === 'false' ? false : null };
          },
        },
      ],
      toDOM: (n): DOMOutputSpec =>
        n.attrs.checked === null
          ? ['li', 0]
          : [
              'li',
              { class: 'md-task', 'data-checked': String(n.attrs.checked) },
              ['span', { class: 'md-task-box', contenteditable: 'false', role: 'checkbox', 'aria-checked': String(n.attrs.checked), 'aria-label': n.attrs.checked ? 'Done — click to uncheck' : 'Not done — click to check' }],
              ['div', { class: 'md-task-body' }, 0],
            ],
    },
    table: block({
      content: 'tableRow+',
      tableRole: 'table',
      isolating: true,
      // `delim`: the original delimiter row (`|:--|--:|`), kept while the columns and alignments match.
      attrs: { delim: { default: null } },
      parseDOM: [{ tag: 'table' }],
      toDOM: () => ['div', { class: 'md-table-wrap' }, ['table', ['tbody', 0]]],
    }),
    tableRow: { content: '(tableCell | tableHeader)+', tableRole: 'row', attrs: { src: { default: null } }, parseDOM: [{ tag: 'tr' }], toDOM: () => ['tr', 0] },
    tableHeader: cell('th', 'header_cell'),
    tableCell: cell('td', 'cell'),
    // Raw Markdown/HTML the editor doesn't model (HTML blocks, link reference definitions): edited as source.
    rawBlock: block({
      content: 'text*',
      marks: '',
      code: true,
      defining: true,
      attrs: { kind: { default: 'html' } },
      parseDOM: [],
      toDOM: (n) => ['pre', { class: `md-raw md-raw-${n.attrs.kind}` }, ['code', 0]],
    }),
    // YAML front matter (only first in a document).
    frontMatter: block({
      content: 'text*',
      marks: '',
      code: true,
      defining: true,
      attrs: { open: { default: '---' }, close: { default: '---' } },
      parseDOM: [],
      toDOM: () => ['pre', { class: 'md-front-matter' }, ['code', 0]],
    }),
    text: { group: 'inline' },
    // A line break inside a paragraph's source: shown as a space, written back as the newline.
    softBreak: {
      inline: true,
      group: 'inline',
      selectable: false,
      leafText: () => ' ',
      parseDOM: [],
      toDOM: () => ['span', { class: 'md-sb' }, ' '],
    },
    hardBreak: {
      inline: true,
      group: 'inline',
      selectable: false,
      // `\` + newline or two trailing spaces, as written.
      attrs: { raw: { default: '\\\n' } },
      leafText: () => '\n',
      parseDOM: [{ tag: 'br' }],
      toDOM: () => ['br'],
    },
    image: {
      inline: true,
      group: 'inline',
      draggable: true,
      attrs: { src: { default: '' }, alt: { default: '' }, title: { default: null }, raw: { default: null } },
      parseDOM: [{ tag: 'img[src]', getAttrs: (dom) => ({ src: (dom as HTMLElement).getAttribute('src') ?? '', alt: (dom as HTMLElement).getAttribute('alt') ?? '', title: (dom as HTMLElement).getAttribute('title') }) }],
      toDOM: (n) => ['img', { src: n.attrs.src, alt: n.attrs.alt, title: n.attrs.title }],
    },
    // `[Label](devall:kind/id)`: shown as a live chip; `label` is the link text's Markdown source.
    devallLink: {
      inline: true,
      group: 'inline',
      atom: true,
      draggable: true,
      attrs: { href: { default: '' }, label: { default: '' }, title: { default: null }, raw: { default: null } },
      leafText: (n) => n.attrs.label,
      parseDOM: [
        {
          tag: 'a[href]',
          priority: 60,
          getAttrs: (dom) => {
            const href = (dom as HTMLElement).getAttribute('href') ?? '';
            return /^devall:/i.test(href) ? { href, label: (dom as HTMLElement).getAttribute('data-dl-label') ?? (dom as HTMLElement).textContent ?? '' } : false;
          },
        },
      ],
      toDOM: (n) => ['a', { href: n.attrs.href, class: 'md-devall', 'data-dl-label': n.attrs.label }, n.attrs.label],
    },
    // Inline HTML (`<kbd>`, `<br>`, comments): kept as written.
    htmlInline: {
      inline: true,
      group: 'inline',
      atom: true,
      attrs: { raw: { default: '' } },
      leafText: (n) => n.attrs.raw,
      parseDOM: [],
      toDOM: (n) => ['code', { class: 'md-html-inline', title: 'Inline HTML' }, n.attrs.raw],
    },
  },
  marks: {
    // `style`: inline `[x](url)`, `angle` `<url>`, `bare` (GFM autolink), `ref` `[x][label]`, `collapsed` `[x][]`,
    // `shortcut` `[x]`. `raw` is written back while the link text still means `key` (see semantic.ts).
    link: {
      inclusive: false,
      attrs: {
        href: { default: '' },
        title: { default: null },
        style: { default: 'inline' },
        ref: { default: null },
        raw: { default: null },
        key: { default: null },
        dest: { default: null },
        titleRaw: { default: null },
      },
      parseDOM: [{ tag: 'a[href]', getAttrs: (dom) => ({ href: (dom as HTMLElement).getAttribute('href'), title: (dom as HTMLElement).getAttribute('title') }) }],
      toDOM: (m) => ['a', { href: m.attrs.href, title: m.attrs.title, class: 'md-link' }, 0],
    },
    strong: {
      attrs: { marker: { default: '**' } },
      parseDOM: [{ tag: 'strong' }, { tag: 'b', getAttrs: (dom) => (dom as HTMLElement).style.fontWeight !== 'normal' && null }, { style: 'font-weight=700' }, { style: 'font-weight=bold' }],
      toDOM: () => ['strong', 0],
    },
    em: {
      attrs: { marker: { default: '*' } },
      parseDOM: [{ tag: 'em' }, { tag: 'i' }, { style: 'font-style=italic' }],
      toDOM: () => ['em', 0],
    },
    strike: {
      attrs: { marker: { default: '~~' } },
      parseDOM: [{ tag: 's' }, { tag: 'del' }, { tag: 'strike' }, { style: 'text-decoration=line-through' }],
      toDOM: () => ['del', 0],
    },
    code: {
      attrs: { raw: { default: null } },
      parseDOM: [{ tag: 'code' }],
      toDOM: () => ['code', 0],
    },
    // A character written backslash-escaped (`\*`): kept escaped when the paragraph is re-written.
    escape: { inclusive: false, parseDOM: [], toDOM: () => ['span', 0] },
    // An entity reference (`&nbsp;`, `&#123;`) shown as its character.
    entity: { inclusive: false, attrs: { raw: { default: '' } }, parseDOM: [], toDOM: () => ['span', { class: 'md-entity' }, 0] },
  },
});

export type MdNode = PMNode;
export type MdMark = Mark;

export const N = schema.nodes;
export const M = schema.marks;

/** Marks that are written as delimiters around text (in nesting order, outermost first). */
export const DELIMITER_MARKS = new Set(['link', 'strong', 'em', 'strike', 'code']);
