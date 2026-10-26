import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { EditorState, TextSelection } from 'prosemirror-state';
import type { Node as PMNode } from 'prosemirror-model';
import { describe, expect, it } from 'vitest';
import { parseFragment, parseMarkdown } from './parse';
import { serializeMarkdown } from './serialize';
import { sameMeaning } from './semantic';
import { schema, N } from './schema';
import { insertDevallLink, linkAt, setLink, toggleTask } from './commands';
import { toggleTaskInSource } from './tasks';

const REPO = join(__dirname, '../../../../..');

const corpus: [string, string][] = [
  ...readdirSync(join(REPO, 'docs'))
    .filter((f) => f.endsWith('.md'))
    .map((f): [string, string] => [`docs/${f}`, readFileSync(join(REPO, 'docs', f), 'utf8')]),
  ['README.md', readFileSync(join(REPO, 'README.md'), 'utf8')],
  ...readdirSync(join(REPO, 'examples/jsonplaceholder/notes'))
    .filter((f) => f.endsWith('.md'))
    .map((f): [string, string] => [`examples/…/${f}`, readFileSync(join(REPO, 'examples/jsonplaceholder/notes', f), 'utf8')]),
];

/** Every construct the editor knows, written in less common styles. */
export const SYNTHETIC = [
  '---',
  'title: Synthetic',
  'tags: [a, b]',
  '---',
  '',
  'Setext heading',
  '==============',
  '',
  '## ATX heading with closing ##',
  '',
  'A paragraph with *em*, _em_, **strong**, __strong__, ~~strike~~, ~single~, `code`, `` a ` tick ``,',
  'a soft break, an escaped \\* star, an entity &amp; &copy; &#123;, inline <kbd>Ctrl</kbd> HTML  ',
  'after a hard break and a backslash break\\',
  'here.',
  '',
  'Links: [inline](https://example.com "Title"), [angle](<a b.md>), <https://auto.link>, https://bare.link,',
  '[full ref][Ref], [collapsed][], [shortcut], ![image *alt*](img/x.png "t") and',
  '[Create order](devall:request/orders/create.req.yaml) plus **[bold link](devall:note/a%20b.md)**.',
  '',
  '[ref]: https://example.com/ref "Ref title"',
  '[collapsed]: https://example.com/c',
  '[shortcut]: <https://example.com/s>',
  '',
  '* star bullet',
  '* second',
  '    * nested four-space',
  '',
  '+ plus list',
  '',
  '1) paren ordered',
  '2) second',
  '',
  '3. starts at three',
  '3. same number',
  '',
  '- [ ] open task',
  '- [x] done task',
  '- [X] capital X',
  '  - [ ] nested task',
  '',
  '- loose item',
  '',
  '  second paragraph',
  '',
  '- another',
  '',
  '> quote line',
  'lazy continuation',
  '>',
  '> > nested quote',
  '',
  '```ts title="x.ts"',
  'const a = 1;',
  '```',
  '',
  '~~~~',
  '```',
  'fence in fence',
  '```',
  '~~~~',
  '',
  '    indented code',
  '    block',
  '',
  '| Left | Center | Right | None |',
  '|:-----|:------:|------:|------|',
  '| a    |   b    |     c | \\| pipe |',
  '| `x`  | **y**  |       | [z](devall:todo/01ABC) |',
  '',
  '<details>',
  '<summary>HTML block</summary>',
  '',
  'Inside.',
  '',
  '</details>',
  '',
  '<!-- a comment -->',
  '',
  '***',
  '',
  '- - -',
  '',
  'Trailing text with trailing spaces   ',
  '',
  '',
  '',
  'Last line without newline',
].join('\n');

const crlf = (s: string) => s.replace(/\r?\n/g, '\r\n');

/** A structurally equal copy (no node identity shared with the parse). */
const rebuild = (doc: PMNode) => schema.nodeFromJSON(doc.toJSON());

function edit(md: string, f: (state: EditorState) => EditorState): string {
  const { doc, source } = parseMarkdown(md);
  const state = f(EditorState.create({ doc }));
  return serializeMarkdown(state.doc, source);
}

/** Position just inside the start of the first textblock whose text includes `needle`. */
function posOf(doc: PMNode, needle: string): number {
  let found = -1;
  doc.descendants((n, pos) => {
    if (found >= 0) return false;
    if (n.isTextblock && n.textContent.includes(needle)) {
      found = pos + 1;
      return false;
    }
    return true;
  });
  if (found < 0) throw new Error(`not found: ${needle}`);
  return found;
}

/** Lines that differ between two texts (same line count assumed). */
function changedLines(a: string, b: string): string[] {
  const x = a.split('\n');
  const y = b.split('\n');
  const out: string[] = [];
  for (let i = 0; i < Math.max(x.length, y.length); i++) if (x[i] !== y[i]) out.push(`${i + 1}: ${JSON.stringify(x[i])} → ${JSON.stringify(y[i])}`);
  return out;
}

describe('round trip', () => {
  const all: [string, string][] = [...corpus, ['synthetic', SYNTHETIC], ['synthetic (CRLF)', crlf(SYNTHETIC)], ['synthetic (BOM)', `﻿${SYNTHETIC}\n`]];
  it.each(all)('%s: unedited → identical', (_name, md) => {
    const { doc, source } = parseMarkdown(md);
    expect(serializeMarkdown(doc, source)).toBe(md);
  });
  it.each(all)('%s: re-created nodes → identical (sources and separators kept per block)', (_name, md) => {
    const { doc, source } = parseMarkdown(md);
    expect(serializeMarkdown(rebuild(doc), source)).toBe(md);
  });
  it.each(all)('%s: written from scratch → same meaning', (_name, md) => {
    const { doc } = parseMarkdown(md);
    const fresh = serializeMarkdown(rebuild(doc), null);
    const back = parseMarkdown(fresh).doc;
    expect(back.childCount).toBe(doc.childCount);
    doc.forEach((n, _o, i) => {
      const ok = sameMeaning(n, back.child(i));
      if (!ok) console.log('differs', JSON.stringify(n.textContent.slice(0, 80)), '\n', JSON.stringify(back.child(i).toJSON()).slice(0, 400), '\n', JSON.stringify(n.toJSON()).slice(0, 400));
      expect(ok).toBe(true);
    });
  });
});

describe('edits stay local', () => {
  it.each([...corpus, ['synthetic (CRLF)', crlf(SYNTHETIC)] as [string, string]])('%s: editing any one block re-writes only that block', (_name, md) => {
    const { doc, source } = parseMarkdown(md);
    const original = source.text;
    doc.forEach((block, offset) => {
      // The first textblock inside this block.
      let at = block.isTextblock ? offset + 1 : -1;
      block.descendants((n, pos) => {
        if (at >= 0) return false;
        if (n.isTextblock) at = offset + 1 + pos + 1;
        return at < 0;
      });
      if (at < 0) return;
      const state = EditorState.create({ doc });
      const out = serializeMarkdown(state.apply(state.tr.insertText('X', at)).doc, source);
      const top = source.top.get(block)!;
      const before = (source.bom ? '﻿' : '') + source.slice(0, top.start);
      const after = source.slice(top.srcEnd, source.lf.length);
      expect(out.startsWith(before)).toBe(true);
      expect(out.endsWith(after)).toBe(true);
      expect(out.length).toBeGreaterThan(original.length - 1);
    });
  });

  it('typing in a paragraph changes only its lines', () => {
    const out = edit(SYNTHETIC, (s) => s.apply(s.tr.insertText('NEW ', posOf(s.doc, 'Inside.'))));
    expect(changedLines(SYNTHETIC, out)).toEqual(['73: "Inside." → "NEW Inside."']);
  });

  it('typing in a list item keeps the other items and the markers', () => {
    const out = edit(SYNTHETIC, (s) => s.apply(s.tr.insertText('!', posOf(s.doc, 'second') + 'second'.length)));
    expect(changedLines(SYNTHETIC, out)).toEqual(['25: "* second" → "* second!"']);
  });

  it('typing in a styled paragraph keeps its markers, escapes, entities and links', () => {
    const md = 'Para with _em_, __strong__, \\* star, &amp; and [ref][Ref] and <https://a.b>.\n\n[ref]: https://x\n';
    const out = edit(md, (s) => s.apply(s.tr.insertText('X', posOf(s.doc, 'Para') + 4)));
    expect(out).toBe('ParaX with _em_, __strong__, \\* star, &amp; and [ref][Ref] and <https://a.b>.\n\n[ref]: https://x\n');
  });

  it('keeps CRLF line endings, re-writing only the edited block', () => {
    const md = crlf(SYNTHETIC);
    const out = edit(md, (s) => s.apply(s.tr.insertText('NEW ', posOf(s.doc, 'Inside.'))));
    expect(out).toBe(md.replace('Inside.', 'NEW Inside.'));
  });

  it('a new paragraph at the end is separated by a blank line', () => {
    const md = 'First\n';
    const out = edit(md, (s) => s.apply(s.tr.insert(s.doc.content.size, N.paragraph.create(null, schema.text('Second')))));
    expect(out).toBe('First\n\nSecond\n');
  });

  it('deleting a block keeps the others byte for byte', () => {
    const md = '# A\n\nOne\n\nTwo\n\n- x\n- y\n';
    const out = edit(md, (s) => {
      const from = posOf(s.doc, 'One') - 1;
      return s.apply(s.tr.delete(from, from + s.doc.resolve(from + 1).parent.nodeSize));
    });
    expect(out).toBe('# A\n\nTwo\n\n- x\n- y\n');
  });

  it('bold in an edited paragraph is written with the file style', () => {
    const md = 'plain words here\n';
    const out = edit(md, (s) => {
      const p = posOf(s.doc, 'plain');
      return s.apply(s.tr.addMark(p + 6, p + 11, schema.marks.strong.create()));
    });
    expect(out).toBe('plain **words** here\n');
  });

  it('escapes only what would be read as Markdown', () => {
    const doc = N.doc.create(null, [N.paragraph.create(null, schema.text('snake_case 2 * 3 a_b *x* [1] [y](z) # not heading <div> &amp;'))]);
    expect(serializeMarkdown(doc)).toBe('snake_case 2 * 3 a_b \\*x\\* [1] \\[y](z) # not heading \\<div> \\&amp;');
    const lineStart = N.doc.create(null, [N.paragraph.create(null, schema.text('# hash')), N.paragraph.create(null, schema.text('1. one')), N.paragraph.create(null, schema.text('- dash'))]);
    expect(serializeMarkdown(lineStart)).toBe('\\# hash\n\n1\\. one\n\n\\- dash');
  });
});

describe('task lists', () => {
  it('toggling a checkbox in the editor changes only that line', () => {
    const out = edit(SYNTHETIC, (s) => {
      const p = posOf(s.doc, 'open task');
      return s.apply(toggleTask(s, p)!);
    });
    expect(changedLines(SYNTHETIC, out)).toEqual(['36: "- [ ] open task" → "- [x] open task"']);
  });

  it('toggling a nested task changes only that line', () => {
    const out = edit(SYNTHETIC, (s) => s.apply(toggleTask(s, posOf(s.doc, 'nested task'))!));
    expect(changedLines(SYNTHETIC, out)).toEqual(['39: "  - [ ] nested task" → "  - [x] nested task"']);
  });

  it('unchecking keeps CRLF and everything else', () => {
    const md = crlf('Intro\n\n- [x] a\n- [ ] b\n');
    const out = edit(md, (s) => s.apply(toggleTask(s, posOf(s.doc, 'a'))!));
    expect(out).toBe(crlf('Intro\n\n- [ ] a\n- [ ] b\n'));
  });

  it('toggleTaskInSource flips the nth checkbox of rendered Markdown', () => {
    const md = 'x\n\n- [ ] one\n  - [x] two\n\n```\n- [ ] not a task\n```\n\n> - [ ] three\n';
    expect(toggleTaskInSource(md, 1)).toBe(md.replace('[x] two', '[ ] two'));
    expect(toggleTaskInSource(md, 2)).toBe(md.replace('> - [ ] three', '> - [x] three'));
    expect(toggleTaskInSource(md, 5)).toBe(md);
  });
});

describe('links', () => {
  it('[[ inserts [Title](devall:…) at the caret', () => {
    const md = 'See  for details\n';
    const out = edit(md, (s) => {
      const at = posOf(s.doc, 'See') + 4;
      const s2 = s.apply(s.tr.setSelection(TextSelection.create(s.doc, at)));
      return s2.apply(insertDevallLink(s2, { link: 'devall:note/design/auth.md', title: 'Auth (design)' }));
    });
    expect(out).toBe('See [Auth (design)](devall:note/design/auth.md) for details\n');
  });

  it('a selection becomes the link text', () => {
    const md = 'Read the runbook today\n';
    const out = edit(md, (s) => {
      const p = posOf(s.doc, 'Read');
      const s2 = s.apply(s.tr.setSelection(TextSelection.create(s.doc, p + 9, p + 16)));
      return s2.apply(insertDevallLink(s2, { link: 'devall:note/ops/runbook.md', title: 'Runbook' }));
    });
    expect(out).toBe('Read the [runbook](devall:note/ops/runbook.md) today\n');
  });

  it('devall: links are chips that keep their Markdown', () => {
    const { doc } = parseMarkdown('[Create order](devall:request/orders/create.req.yaml)\n');
    const chip = doc.firstChild!.firstChild!;
    expect(chip.type.name).toBe('devallLink');
    expect(chip.attrs.href).toBe('devall:request/orders/create.req.yaml');
    expect(chip.attrs.label).toBe('Create order');
  });

  it('editing a link’s URL re-writes only that link', () => {
    const md = 'See [the docs](<https://a.b/x y> "T") and [other](https://o).\n';
    const out = edit(md, (s) => {
      const at = posOf(s.doc, 'See') + 6;
      const s2 = s.apply(s.tr.setSelection(TextSelection.create(s.doc, at)));
      expect(linkAt(s2)).toMatchObject({ href: 'https://a.b/x y', from: posOf(s.doc, 'See') + 4 });
      return s2.apply(setLink(s2, 'https://new.example/'));
    });
    expect(out).toBe('See [the docs](https://new.example/) and [other](https://o).\n');
  });

  it('parses pasted Markdown with the document’s definitions', () => {
    const frag = parseFragment('[a][r]', { r: { href: 'https://r' } });
    expect(frag.firstChild!.firstChild!.marks[0].attrs.href).toBe('https://r');
  });
});
