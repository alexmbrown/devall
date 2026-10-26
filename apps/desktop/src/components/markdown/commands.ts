import { lift, setBlockType, toggleMark, wrapIn } from 'prosemirror-commands';
import type { Attrs, MarkType, Node as PMNode, NodeType } from 'prosemirror-model';
import { liftListItem, wrapInList } from 'prosemirror-schema-list';
import { NodeSelection, TextSelection, type Command, type EditorState, type Transaction } from 'prosemirror-state';
import { escapeMarkdownLabel } from '@/lib/links/syntax';
import { M, N } from './schema';

/**
 * Editor commands that don't need a view (unit-tested): formatting, block types, task items,
 * links. Commands that change a block's type never carry its `src` over (it's another block now).
 */

/** The list item around `pos` (or the selection), with its position. */
export function itemAt(state: EditorState, pos?: number): { node: PMNode; pos: number } | null {
  const $p = state.doc.resolve(pos ?? state.selection.from);
  for (let d = $p.depth; d > 0; d--) {
    const n = $p.node(d);
    if (n.type === N.listItem) return { node: n, pos: $p.before(d) };
  }
  return null;
}

/** Checks/unchecks the task item at `pos` (or the selection); null when it isn't in one. */
export function toggleTask(state: EditorState, pos?: number): Transaction | null {
  const item = itemAt(state, pos);
  if (!item || item.node.attrs.checked === null) return null;
  return state.tr.setNodeMarkup(item.pos, undefined, { ...item.node.attrs, checked: !item.node.attrs.checked });
}

/** Mod-Enter in a task item. */
export const toggleTaskCommand: Command = (state, dispatch) => {
  const tr = toggleTask(state);
  if (!tr) return false;
  dispatch?.(tr);
  return true;
};

/** Turns the list items in the selection into task items (or back), wrapping paragraphs in a list first. */
export const toggleTaskList: Command = (state, dispatch) => {
  const item = itemAt(state);
  if (!item) {
    if (!wrapInList(N.bulletList)(state)) return false;
    if (dispatch) {
      wrapInList(N.bulletList)(state, (tr) => {
        const next = state.apply(tr);
        const it = itemAt(next);
        if (it) tr.setNodeMarkup(it.pos, undefined, { ...it.node.attrs, checked: false });
        dispatch(tr);
      });
    }
    return true;
  }
  if (dispatch) {
    const on = item.node.attrs.checked === null;
    const tr = state.tr;
    const { from, to } = state.selection;
    state.doc.nodesBetween(from, to, (n, pos) => {
      if (n.type !== N.listItem) return true;
      tr.setNodeMarkup(pos, undefined, { ...n.attrs, checked: on ? false : null });
      return true;
    });
    dispatch(tr);
  }
  return true;
};

/** Paragraph ⇄ heading of `level` (same level again → paragraph). */
export function toggleHeading(level: number): Command {
  return (state, dispatch) => {
    const { $from } = state.selection;
    const cur = $from.parent;
    if (cur.type === N.heading && cur.attrs.level === level) return setBlockType(N.paragraph)(state, dispatch);
    return setBlockType(N.heading, { level })(state, dispatch);
  };
}

/** Wraps the selection in a list of `type`, or lifts it out when it's already in one. */
export function toggleList(type: NodeType, attrs?: Attrs): Command {
  return (state, dispatch) => {
    const { $from } = state.selection;
    for (let d = $from.depth; d > 0; d--) {
      const n = $from.node(d);
      if (n.type === N.bulletList || n.type === N.orderedList) {
        if (n.type === type) return liftListItem(N.listItem)(state, dispatch);
        if (dispatch) dispatch(state.tr.setNodeMarkup($from.before(d), type, { ...(type === N.orderedList ? { start: 1, delim: '.' } : { bullet: '-' }), tight: n.attrs.tight }));
        return true;
      }
    }
    return wrapInList(type, attrs)(state, dispatch);
  };
}

export const toggleQuote: Command = (state, dispatch) => {
  const { $from } = state.selection;
  for (let d = $from.depth; d > 0; d--) if ($from.node(d).type === N.blockquote) return lift(state, dispatch);
  return wrapIn(N.blockquote)(state, dispatch);
};

export const setParagraph: Command = (state, dispatch) => setBlockType(N.paragraph)(state, dispatch);

export function setCodeBlock(info = ''): Command {
  return (state, dispatch) => setBlockType(N.codeBlock, { info })(state, dispatch);
}

export const insertRule: Command = (state, dispatch) => {
  if (dispatch) {
    const { $from } = state.selection;
    const hr = N.horizontalRule.create();
    const tr = state.tr;
    // On an empty paragraph: replace it; otherwise after the current block.
    if ($from.parent.type === N.paragraph && $from.parent.childCount === 0 && $from.depth >= 1) tr.replaceWith($from.before(), $from.after(), [hr, N.paragraph.create()]);
    else tr.insert($from.after(1), [hr, N.paragraph.create()]);
    const after = tr.mapping.map($from.pos);
    tr.setSelection(TextSelection.near(tr.doc.resolve(Math.min(after + 2, tr.doc.content.size))));
    dispatch(tr.scrollIntoView());
  }
  return true;
};

/** A `rows` × `cols` table (first row: headers) where the selection is. */
export function insertTable(rows = 3, cols = 3): Command {
  return (state, dispatch) => {
    if (dispatch) {
      const row = (header: boolean) => N.tableRow.create(null, Array.from({ length: cols }, () => (header ? N.tableHeader : N.tableCell).create()));
      const table = N.table.create(null, [row(true), ...Array.from({ length: rows - 1 }, () => row(false))]);
      const { $from } = state.selection;
      const tr = state.tr;
      const empty = $from.parent.type === N.paragraph && $from.parent.childCount === 0 && $from.depth >= 1;
      const at = empty ? $from.before() : $from.after(Math.max(1, $from.depth));
      if (empty) tr.replaceWith($from.before(), $from.after(), table);
      else tr.insert(at, table);
      tr.setSelection(TextSelection.near(tr.doc.resolve(at + 3)));
      dispatch(tr.scrollIntoView());
    }
    return true;
  };
}

export const toggleBold = toggleMark(M.strong);
export const toggleItalic = toggleMark(M.em);
export const toggleStrike = toggleMark(M.strike);
export const toggleCode = toggleMark(M.code);

/** Whether `type` is active on the selection (for toolbar states). */
export function markActive(state: EditorState, type: MarkType): boolean {
  const { from, $from, to, empty } = state.selection;
  if (empty) return !!type.isInSet(state.storedMarks || $from.marks());
  return state.doc.rangeHasMark(from, to, type);
}

/** The link mark at the selection, with its range. */
export function linkAt(state: EditorState): { href: string; from: number; to: number; title: string | null } | null {
  const { $from } = state.selection;
  const mark = ($from.nodeAfter && M.link.isInSet($from.nodeAfter.marks)) || ($from.nodeBefore && M.link.isInSet($from.nodeBefore.marks));
  if (!mark) return null;
  const start = $from.start();
  // The run of children carrying this link that contains the caret.
  let from = -1;
  let to = -1;
  let found = false;
  $from.parent.forEach((child, offset) => {
    if (found) return;
    const a = start + offset;
    if (mark.isInSet(child.marks)) {
      if (from < 0) from = a;
      to = a + child.nodeSize;
    } else {
      if (from >= 0 && from <= $from.pos && to >= $from.pos) found = true;
      else from = to = -1;
    }
  });
  if (from < 0 || $from.pos < from || $from.pos > to) return null;
  return { href: mark.attrs.href, title: mark.attrs.title, from, to };
}

/** Sets (or, with an empty href, removes) an ordinary link on the selection / the link at the caret. */
export function setLink(state: EditorState, href: string, text?: string): Transaction {
  const tr = state.tr;
  const existing = linkAt(state);
  let { from, to } = state.selection;
  if (from === to && existing) ({ from, to } = existing);
  tr.removeMark(from, to, M.link);
  if (!href.trim()) return tr;
  const mark = M.link.create({ href: href.trim() });
  if (from === to) {
    const label = text?.trim() || href.trim();
    tr.insert(from, state.schema.text(label, [mark, ...state.selection.$from.marks().filter((m) => m.type !== M.link)]));
    return tr.setSelection(TextSelection.create(tr.doc, from + label.length));
  }
  return tr.addMark(from, to, mark);
}

/**
 * Inserts a `devall:` link as a chip at the selection (`[[`, Ctrl/Cmd+Alt+K, the Link button):
 * a selection inside one block becomes its text, else the item's title.
 */
export function insertDevallLink(state: EditorState, picked: { link: string; title: string }, range?: { from: number; to: number }): Transaction {
  const from = range?.from ?? state.selection.from;
  const to = range?.to ?? state.selection.to;
  const selected = range ? '' : state.doc.textBetween(from, to, ' ').trim();
  const label = escapeMarkdownLabel(selected && !selected.includes('\n') ? selected : picked.title) || picked.link;
  const marks = state.doc.resolve(from).marks().filter((m) => m.type !== M.link && m.type !== M.code && m.type !== M.escape && m.type !== M.entity);
  const chip = N.devallLink.create({ href: picked.link, label }, undefined, marks);
  const tr = state.tr.replaceWith(from, to, chip);
  return tr.setSelection(TextSelection.create(tr.doc, from + chip.nodeSize)).scrollIntoView();
}

/** Changes a chip's target (keeps its text unless it was the old item's title). */
export function retargetDevallLink(state: EditorState, pos: number, picked: { link: string; title: string }, keepLabel: boolean): Transaction {
  const node = state.doc.nodeAt(pos)!;
  return state.tr.setNodeMarkup(pos, undefined, { ...node.attrs, href: picked.link, label: keepLabel ? node.attrs.label : escapeMarkdownLabel(picked.title), raw: null });
}

/** The chip selected (node selection) or right before the caret. */
export function selectedChip(state: EditorState): { node: PMNode; pos: number } | null {
  const sel = state.selection;
  if (sel instanceof NodeSelection && sel.node.type === N.devallLink) return { node: sel.node, pos: sel.from };
  return null;
}
