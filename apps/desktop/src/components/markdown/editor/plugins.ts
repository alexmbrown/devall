import { baseKeymap, chainCommands, exitCode, setBlockType } from 'prosemirror-commands';
import { redo, undo } from 'prosemirror-history';
import { InputRule, inputRules, textblockTypeInputRule, undoInputRule, wrappingInputRule } from 'prosemirror-inputrules';
import { keymap } from 'prosemirror-keymap';
import type { Attrs, MarkType, Node as PMNode } from 'prosemirror-model';
import { liftListItem, sinkListItem, splitListItem } from 'prosemirror-schema-list';
import { Plugin, PluginKey, TextSelection, type Command, type EditorState } from 'prosemirror-state';
import { goToNextCell } from 'prosemirror-tables';
import { findWrapping } from 'prosemirror-transform';
import { Decoration, DecorationSet, type EditorView } from 'prosemirror-view';
import { itemAt, setParagraph, toggleBold, toggleCode, toggleHeading, toggleItalic, toggleList, toggleStrike, toggleTaskCommand, toggleTaskList } from '../commands';
import { M, N } from '../schema';

/** What the plugins need from the React side (always the latest props). */
export interface PluginHost {
  /** Opens the "Link to…" picker; `range` is replaced by the pick (else the selection). */
  pickLink(range?: { from: number; to: number }): void;
  onSubmit?(): boolean;
  onEscape?(): boolean;
  onSave?(): boolean;
  /** Keys while the slash menu is open (true: handled). */
  slashKey(key: string): boolean;
  placeholder(): string | undefined;
}

// ── input rules: Markdown shortcuts while typing ───────────────────────────────────────────────

/** `**x**` → strong etc.: the typed delimiters disappear, the text gets the mark. */
function markRule(re: RegExp, type: MarkType, attrs: (m: RegExpMatchArray) => Attrs): InputRule {
  return new InputRule(re, (state, match, start, end) => {
    const full = match[1];
    const inner = match[2];
    const from = start + match[0].lastIndexOf(full);
    // The last delimiter character was just typed: it isn't in the document yet.
    const to = end;
    const tr = state.tr.delete(from, to);
    const marks = state.doc.resolve(from).marks();
    tr.insert(from, state.schema.text(inner, type.create(attrs(match)).addToSet(marks)));
    tr.removeStoredMark(type);
    return tr;
  });
}

/** `[ ] ` / `[x] ` at the start of a list item makes it a task; in a paragraph, a task list. */
const taskRule = new InputRule(/^\[( |x|X)?\]\s$/, (state, match, start, end) => {
  const checked = !!match[1] && match[1] !== ' ';
  const $s = state.doc.resolve(start);
  if ($s.parent.type !== N.paragraph) return null;
  if ($s.depth >= 2 && $s.node(-1).type === N.listItem) {
    if ($s.index(-1) !== 0) return null;
    const item = itemAt(state, start)!;
    return state.tr.delete(start, end).setNodeMarkup(item.pos, undefined, { ...item.node.attrs, checked });
  }
  const tr = state.tr.delete(start, end);
  const range = tr.doc.resolve(start).blockRange();
  const wrapping = range && findWrapping(range, N.bulletList);
  if (!range || !wrapping) return null;
  return tr.wrap(
    range,
    wrapping.map((w) => (w.type === N.listItem ? { type: N.listItem, attrs: { checked } } : w)),
  );
});

const fenceRule = new InputRule(/^(```|~~~)([\w+#.-]*)\s$/, (state, match, start, end) => {
  const $s = state.doc.resolve(start);
  if (!$s.node(-1).canReplaceWith($s.index(-1), $s.indexAfter(-1), N.codeBlock)) return null;
  return state.tr.delete(start, end).setBlockType(start, start, N.codeBlock, { info: match[2] ?? '', fence: match[1] });
});

const hrRule = new InputRule(/^(?:---|\*\*\*|___)$/, (state, _m, start, end) => {
  const $s = state.doc.resolve(start);
  if ($s.parent.type !== N.paragraph || $s.parent.textContent.length > 2) return null;
  const tr = state.tr.replaceWith($s.before(), $s.after(), [N.horizontalRule.create(), N.paragraph.create()]);
  tr.setSelection(TextSelection.near(tr.doc.resolve($s.before() + 2)));
  void end;
  return tr;
});

export function markdownInputRules() {
  return inputRules({
    rules: [
      textblockTypeInputRule(/^(#{1,6})\s$/, N.heading, (m) => ({ level: m[1].length })),
      wrappingInputRule(/^\s*>\s$/, N.blockquote),
      wrappingInputRule(/^\s*([-+*])\s$/, N.bulletList, (m) => ({ bullet: m[1] })),
      wrappingInputRule(
        /^(\d{1,9})([.)])\s$/,
        N.orderedList,
        (m) => ({ start: +m[1], delim: m[2] }),
        (m, node) => node.childCount + (node.attrs.start as number) === +m[1],
      ),
      taskRule,
      fenceRule,
      hrRule,
      markRule(/(?:^|[^*\\])(\*\*([^*\s](?:[^*]*[^*\s])?)\*\*)$/, M.strong, () => ({ marker: '**' })),
      markRule(/(?:^|[^_\\\p{L}\p{N}])(__([^_\s](?:[^_]*[^_\s])?)__)$/u, M.strong, () => ({ marker: '__' })),
      markRule(/(?:^|[^*\\])(\*([^*\s](?:[^*]*[^*\s])?)\*)$/, M.em, () => ({ marker: '*' })),
      markRule(/(?:^|[^_\\\p{L}\p{N}])(_([^_\s](?:[^_]*[^_\s])?)_)$/u, M.em, () => ({ marker: '_' })),
      markRule(/(?:^|[^~\\])(~~([^~\s](?:[^~]*[^~\s])?)~~)$/, M.strike, () => ({ marker: '~~' })),
      markRule(/(?:^|[^`\\])(`([^`]+)`)$/, M.code, () => ({})),
    ],
  });
}

// ── keymap ─────────────────────────────────────────────────────────────────────────────────────

const inCode = (state: EditorState) => !!state.selection.$from.parent.type.spec.code;

/** Enter in a list item: a new item (a task item when this one is). */
const splitItem: Command = (state, dispatch) => {
  const item = itemAt(state);
  if (!item) return false;
  return splitListItem(N.listItem, { checked: item.node.attrs.checked === null ? null : false })(state, dispatch);
};

/** Enter after ```lang in a paragraph: a code block. */
const fenceOnEnter: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parent.type !== N.paragraph) return false;
  const m = /^(```|~~~)([\w+#.-]*)$/.exec($from.parent.textContent);
  if (!m || $from.parentOffset !== $from.parent.content.size) return false;
  if (dispatch) {
    const start = $from.start();
    dispatch(state.tr.delete(start, $from.pos).setBlockType(start, start, N.codeBlock, { info: m[2], fence: m[1] }));
  }
  return true;
};

/** Backspace at the very start of a list item: out of the list. */
const liftAtStart: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parentOffset !== 0 || $from.depth < 2) return false;
  if ($from.node(-1).type !== N.listItem || $from.index(-1) !== 0) return false;
  return liftListItem(N.listItem)(state, dispatch);
};

const hardBreak: Command = (state, dispatch) => {
  if (inCode(state)) {
    dispatch?.(state.tr.insertText('\n'));
    return true;
  }
  dispatch?.(state.tr.replaceSelectionWith(N.hardBreak.create()).scrollIntoView());
  return true;
};

const codeTab: Command = (state, dispatch) => {
  if (!inCode(state)) return false;
  dispatch?.(state.tr.insertText('  '));
  return true;
};

const inTable = (state: EditorState) => {
  const { $from } = state.selection;
  for (let d = $from.depth; d > 0; d--) if ($from.node(d).type === N.table) return true;
  return false;
};

/** Ctrl/Cmd+Alt+K ("Mod-Alt-k") is ⌘⌥K on macOS and Ctrl+Alt+K elsewhere, matching `isLinkShortcut`. */
export function markdownKeymap(host: PluginHost) {
  const run = (f?: () => boolean): Command => () => !!f?.();
  const keys: Record<string, Command> = {
    'Mod-z': undo,
    'Shift-Mod-z': redo,
    'Mod-y': redo,
    'Mod-b': toggleBold,
    'Mod-i': toggleItalic,
    'Mod-e': toggleCode,
    'Mod-Shift-x': toggleStrike,
    'Mod-Alt-0': setParagraph,
    'Mod-Alt-1': toggleHeading(1),
    'Mod-Alt-2': toggleHeading(2),
    'Mod-Alt-3': toggleHeading(3),
    'Mod-Alt-4': toggleHeading(4),
    'Mod-Shift-7': toggleList(N.orderedList),
    'Mod-Shift-8': toggleList(N.bulletList),
    'Mod-Shift-9': toggleTaskList,
    'Mod-Alt-c': (state, dispatch) => setBlockType(N.codeBlock)(state, dispatch),
    'Mod-Alt-k': () => {
      host.pickLink();
      return true;
    },
    'Mod-s': run(() => host.onSave?.() ?? false),
    Enter: chainCommands(fenceOnEnter, splitItem),
    'Shift-Enter': hardBreak,
    'Mod-Enter': chainCommands(toggleTaskCommand, exitCode, run(() => host.onSubmit?.() ?? false)),
    Tab: chainCommands(codeTab, (s, d) => (inTable(s) ? goToNextCell(1)(s, d) : false), sinkListItem(N.listItem)),
    'Shift-Tab': chainCommands((s, d) => (inTable(s) ? goToNextCell(-1)(s, d) : false), liftListItem(N.listItem)),
    Backspace: chainCommands(undoInputRule, liftAtStart),
    Escape: run(() => host.onEscape?.() ?? false),
  };
  return [
    // The slash menu sees navigation keys first.
    keymap({
      ArrowDown: () => host.slashKey('ArrowDown'),
      ArrowUp: () => host.slashKey('ArrowUp'),
      Enter: () => host.slashKey('Enter'),
      Tab: () => host.slashKey('Tab'),
      Escape: () => host.slashKey('Escape'),
    }),
    keymap(keys),
    keymap(baseKeymap),
  ];
}

// ── `[[` opens the link picker ──────────────────────────────────────────────────────────────────

export function wikiLinkTrigger(host: PluginHost) {
  return new Plugin({
    props: {
      handleTextInput(view, from, to, text) {
        if (text !== '[' || from !== to) return false;
        const $f = view.state.doc.resolve(from);
        if ($f.parent.type.spec.code || M.code.isInSet($f.marks())) return false;
        const before = $f.parent.textBetween(Math.max(0, $f.parentOffset - 2), $f.parentOffset, undefined, '￼');
        if (!before.endsWith('[') || before.endsWith('[[') || before.endsWith('\\[')) return false;
        host.pickLink({ from: from - 1, to });
        return true;
      },
    },
  });
}

// ── slash menu (state only; the menu is React) ─────────────────────────────────────────────────

export interface SlashState {
  /** Position of the `/`. */
  from: number;
  query: string;
}

export const slashKey = new PluginKey<SlashState | null>('md-slash');

export function slashPlugin() {
  return new Plugin<SlashState | null>({
    key: slashKey,
    state: {
      init: () => null,
      apply(tr, prev, _old, state) {
        const meta = tr.getMeta(slashKey) as { open?: number; close?: boolean } | undefined;
        if (meta?.close) return null;
        let from = meta?.open ?? (prev ? tr.mapping.map(prev.from) : -1);
        if (from < 0) return null;
        const { $from, empty } = state.selection;
        if (!empty || $from.pos <= from || from < $from.start()) return null;
        const text = state.doc.textBetween(from, $from.pos, '\n', '￼');
        if (!text.startsWith('/') || /\s/.test(text.slice(1)) || text.length > 24) return null;
        from = Math.max(from, 0);
        return { from, query: text.slice(1) };
      },
    },
    props: {
      handleTextInput(view, from, to, text) {
        if (text !== '/' || from !== to) return false;
        const $f = view.state.doc.resolve(from);
        if ($f.parent.type.spec.code || $f.parent.type === N.tableCell || $f.parent.type === N.tableHeader) return false;
        const before = $f.parent.textBetween(Math.max(0, $f.parentOffset - 1), $f.parentOffset, undefined, '￼');
        if (before && !/\s/.test(before)) return false;
        view.dispatch(view.state.tr.insertText('/', from, to).setMeta(slashKey, { open: from }));
        return true;
      },
    },
  });
}

// ── placeholder ────────────────────────────────────────────────────────────────────────────────

export function placeholderPlugin(host: PluginHost) {
  return new Plugin({
    props: {
      decorations(state) {
        const doc = state.doc;
        const text = host.placeholder();
        if (!text || doc.childCount !== 1) return null;
        const first = doc.firstChild as PMNode;
        if (first.type !== N.paragraph || first.content.size) return null;
        return DecorationSet.create(doc, [Decoration.node(0, first.nodeSize, { class: 'md-placeholder', 'data-placeholder': text })]);
      },
    },
  });
}

/** Closes the slash menu (after picking or Escape). */
export function closeSlash(view: EditorView) {
  view.dispatch(view.state.tr.setMeta(slashKey, { close: true }));
}
