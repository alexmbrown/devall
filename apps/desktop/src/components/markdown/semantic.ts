import type { Mark, Node as PMNode } from 'prosemirror-model';

/**
 * What a piece of the document *means* in Markdown, ignoring how it was written (markers, escapes,
 * entity spelling, link style, source bookkeeping). Used to check that freshly written Markdown
 * reads back as the same thing, and to recognise untouched link text.
 */

const markKey = (m: Mark): string | null => {
  switch (m.type.name) {
    case 'link':
      return `link(${m.attrs.href}|${m.attrs.title ?? ''})`;
    case 'strong':
    case 'em':
    case 'strike':
    case 'code':
      return m.type.name;
    default:
      return null; // escape, entity: spelling only
  }
};

const marksKey = (marks: readonly Mark[], skip: readonly Mark[]) =>
  marks
    .filter((m) => !m.isInSet(skip))
    .map(markKey)
    .filter(Boolean)
    .sort()
    .join(',');

interface Run {
  k: string;
  t: string;
  /** A leaf (break, image, chip, HTML) rather than text. */
  leaf: string | null;
}

/**
 * A canonical string for inline content (`nodes` of one textblock or one link). `skip`: marks
 * around the whole run (a link's own mark and what encloses it) that don't count.
 */
export function inlineKey(nodes: readonly PMNode[], skip: readonly Mark[] = []): string {
  const runs: Run[] = [];
  const text = (k: string, t: string) => {
    const last = runs[runs.length - 1];
    if (last && last.k === k && last.leaf === null) last.t += t;
    else runs.push({ k, t, leaf: null });
  };
  for (const n of nodes) {
    const k = marksKey(n.marks, skip);
    const name = n.type.name;
    if (n.isText) text(k, n.text ?? '');
    else if (name === 'softBreak') text(k, '\n');
    else if (name === 'hardBreak') runs.push({ k, t: '', leaf: 'br' });
    else if (name === 'image') runs.push({ k, t: '', leaf: `img(${n.attrs.src}|${n.attrs.alt}|${n.attrs.title ?? ''})` });
    else if (name === 'devallLink') runs.push({ k, t: '', leaf: `dl(${n.attrs.href}|${n.attrs.label}|${n.attrs.title ?? ''})` });
    else if (name === 'htmlInline') runs.push({ k, t: '', leaf: `html(${n.attrs.raw})` });
    else runs.push({ k, t: n.textContent, leaf: name });
  }
  // Markdown drops whitespace at the edges of a block and around line breaks, and a trailing hard break.
  while (runs.length && runs[runs.length - 1].leaf === 'br') runs.pop();
  for (const r of runs) if (r.leaf === null) r.t = r.t.replace(/[ \t]*\n[ \t]*/g, '\n');
  if (runs[0]?.leaf === null) runs[0].t = runs[0].t.replace(/^[ \t\n]+/, '');
  const end = runs[runs.length - 1];
  if (end?.leaf === null) end.t = end.t.replace(/[ \t\n]+$/, '');
  return JSON.stringify(runs.filter((r) => r.t !== '' || r.leaf !== null).map((r) => [r.k, r.t, r.leaf]));
}

const children = (n: PMNode) => {
  const out: PMNode[] = [];
  n.forEach((c) => out.push(c));
  return out;
};

/** Whether two blocks mean the same thing (see above). */
export function sameMeaning(a: PMNode, b: PMNode): boolean {
  if (a.type !== b.type) return false;
  switch (a.type.name) {
    case 'heading':
      if (a.attrs.level !== b.attrs.level) return false;
      break;
    case 'listItem':
      if (a.attrs.checked !== b.attrs.checked) return false;
      break;
    case 'orderedList':
      if (a.attrs.start !== b.attrs.start) return false;
      break;
    case 'codeBlock':
      return a.textContent === b.textContent && (a.attrs.info || '').split(/\s/)[0] === (b.attrs.info || '').split(/\s/)[0];
    case 'tableHeader':
    case 'tableCell':
      return inlineKey(children(a)) === inlineKey(children(b));
    case 'rawBlock':
    case 'frontMatter':
      return a.textContent === b.textContent;
  }
  if (a.isTextblock) return inlineKey(children(a)) === inlineKey(children(b));
  if (a.childCount !== b.childCount) return false;
  for (let i = 0; i < a.childCount; i++) if (!sameMeaning(a.child(i), b.child(i))) return false;
  return true;
}
