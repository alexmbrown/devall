import { Transform } from 'prosemirror-transform';
import { parseMarkdown } from './parse';
import { N } from './schema';
import { serializeMarkdown } from './serialize';

/**
 * Checks/unchecks the `index`-th task checkbox (document order, as rendered) in Markdown source;
 * only that item's checkbox changes. The text comes back unchanged when there's no such task.
 */
export function toggleTaskInSource(md: string, index: number, checked?: boolean): string {
  const { doc, source } = parseMarkdown(md);
  let n = -1;
  let at = -1;
  doc.descendants((node, pos) => {
    if (at >= 0) return false;
    if (node.type === N.listItem && node.attrs.checked !== null && ++n === index) {
      at = pos;
      return false;
    }
    return true;
  });
  if (at < 0) return md;
  const item = doc.nodeAt(at)!;
  const next = checked ?? !item.attrs.checked;
  if (next === item.attrs.checked) return md;
  const tr = new Transform(doc).setNodeMarkup(at, undefined, { ...item.attrs, checked: next });
  return serializeMarkdown(tr.doc, source);
}
