import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import {
  AlignCenter,
  AlignLeft,
  AlignRight,
  BetweenHorizontalEnd,
  BetweenHorizontalStart,
  BetweenVerticalEnd,
  BetweenVerticalStart,
  Bold,
  Code,
  ExternalLink,
  Globe,
  Heading1,
  Heading2,
  Heading3,
  Italic,
  Link2,
  List,
  ListOrdered,
  ListTodo,
  Minus,
  Pencil,
  Pilcrow,
  Quote,
  SquareCode,
  Strikethrough,
  Table,
  Trash2,
  Unlink,
} from 'lucide-react';
import { setBlockType } from 'prosemirror-commands';
import type { EditorState } from 'prosemirror-state';
import { addColumnAfter, addColumnBefore, addRowAfter, addRowBefore, deleteColumn, deleteRow, deleteTable, selectedRect } from 'prosemirror-tables';
import type { EditorView } from 'prosemirror-view';
import { isMac } from '@/components/ui/misc';
import { linkShortcutLabel } from '@/lib/links/insert';
import { insertRule, insertTable, linkAt, markActive, setParagraph, toggleBold, toggleCode, toggleHeading, toggleItalic, toggleList, toggleQuote, toggleStrike, toggleTaskList } from '../commands';
import { M, N } from '../schema';
import { closeSlash, slashKey } from './plugins';

const MOD = isMac ? '⌘' : 'Ctrl+';

/** A small bar floating at viewport coordinates (kept on screen), never taking focus. */
function Floating({ x, y, above, children, className, label }: { x: number; y: number; above?: boolean; children: ReactNode; className?: string; label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>({ left: -9999, top: -9999 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const left = Math.max(8, Math.min(x - w / 2, window.innerWidth - w - 8));
    let top = above ? y - h - 8 : y + 8;
    if (above && top < 8) top = y + 28;
    if (!above && top + h > window.innerHeight - 8) top = y - h - 32;
    setStyle({ left, top });
  }, [x, y, above]);
  return createPortal(
    <div ref={ref} className={clsx('md-float', className)} style={style} role="toolbar" aria-label={label} onMouseDown={(e) => e.preventDefault()}>
      {children}
    </div>,
    document.body,
  );
}

function Btn({ on, title, onClick, children, danger }: { on?: boolean; title: string; onClick(): void; children: ReactNode; danger?: boolean }) {
  return (
    <button className={clsx('md-float-btn', on && 'on', danger && 'danger')} title={title} aria-label={title} aria-pressed={on} onMouseDown={(e) => e.preventDefault()} onClick={onClick}>
      {children}
    </button>
  );
}

const run = (view: EditorView, cmd: (s: EditorState, d?: EditorView['dispatch']) => boolean) => {
  cmd(view.state, view.dispatch);
  view.focus();
};

/** Formatting bar over a text selection. */
export function SelectionToolbar({ view, onLink, onPick }: { view: EditorView; onLink(): void; onPick(): void }) {
  const { state } = view;
  const { from, to } = state.selection;
  const a = view.coordsAtPos(from);
  const b = view.coordsAtPos(to);
  return (
    <Floating x={(a.left + b.right) / 2} y={Math.min(a.top, b.top)} above label="Formatting">
      <Btn on={markActive(state, M.strong)} title={`Bold (${MOD}B)`} onClick={() => run(view, toggleBold)}>
        <Bold />
      </Btn>
      <Btn on={markActive(state, M.em)} title={`Italic (${MOD}I)`} onClick={() => run(view, toggleItalic)}>
        <Italic />
      </Btn>
      <Btn on={markActive(state, M.strike)} title={`Strikethrough (${MOD}Shift+X)`} onClick={() => run(view, toggleStrike)}>
        <Strikethrough />
      </Btn>
      <Btn on={markActive(state, M.code)} title={`Inline code (${MOD}E)`} onClick={() => run(view, toggleCode)}>
        <Code />
      </Btn>
      <span className="md-float-sep" />
      <Btn on={markActive(state, M.link)} title="Web link…" onClick={onLink}>
        <Globe />
      </Btn>
      <Btn title={`Link to a request, note, to-do… (${linkShortcutLabel(isMac)})`} onClick={onPick}>
        <Link2 />
      </Btn>
      <span className="md-float-sep" />
      <Btn on={state.selection.$from.parent.type === N.heading && state.selection.$from.parent.attrs.level === 2} title="Heading" onClick={() => run(view, toggleHeading(2))}>
        <Heading2 />
      </Btn>
      <Btn title="Quote" onClick={() => run(view, toggleQuote)}>
        <Quote />
      </Btn>
      <Btn title="Bulleted list" onClick={() => run(view, toggleList(N.bulletList))}>
        <List />
      </Btn>
      <Btn title="Task list" onClick={() => run(view, toggleTaskList)}>
        <ListTodo />
      </Btn>
    </Floating>
  );
}

/** Under the caret in a link: where it goes, edit, remove. */
export function LinkBubble({ view, onOpen, onEdit }: { view: EditorView; onOpen(href: string): void; onEdit(): void }) {
  const link = linkAt(view.state);
  if (!link) return null;
  const c = view.coordsAtPos(link.from);
  return (
    <Floating x={c.left + 120} y={c.bottom} label="Link">
      <button className="md-float-link" title={`${link.href}\nOpen (${MOD}click)`} onMouseDown={(e) => e.preventDefault()} onClick={() => onOpen(link.href)}>
        <ExternalLink />
        <span className="truncate">{link.href}</span>
      </button>
      <Btn title="Edit link" onClick={onEdit}>
        <Pencil />
      </Btn>
      <Btn
        title="Remove link"
        onClick={() => {
          view.dispatch(view.state.tr.removeMark(link.from, link.to, M.link));
          view.focus();
        }}
      >
        <Unlink />
      </Btn>
    </Floating>
  );
}

/** Above a table while the caret is in it. */
export function TableBar({ view }: { view: EditorView }) {
  const { state } = view;
  let rect: ReturnType<typeof selectedRect> | null = null;
  try {
    rect = selectedRect(state);
  } catch {
    return null;
  }
  const dom = view.nodeDOM(rect.tableStart - 1) as HTMLElement | null;
  const box = dom?.getBoundingClientRect();
  if (!box) return null;
  const r = rect;
  const align = (a: string | null) => () => {
    // Alignment is per column in GFM: every cell of the selected columns.
    const tr = state.tr;
    for (let row = 0; row < r.map.height; row++)
      for (let col = r.left; col < r.right; col++) {
        const pos = r.tableStart + r.map.map[row * r.map.width + col];
        const cell = state.doc.nodeAt(pos);
        if (cell) tr.setNodeMarkup(pos, undefined, { ...cell.attrs, align: a });
      }
    view.dispatch(tr);
    view.focus();
  };
  return (
    <Floating x={box.left + Math.min(box.width, 360) / 2} y={box.top} above label="Table">
      <Btn title="Insert row above" onClick={() => run(view, addRowBefore)}>
        <BetweenHorizontalStart />
      </Btn>
      <Btn title="Insert row below" onClick={() => run(view, addRowAfter)}>
        <BetweenHorizontalEnd />
      </Btn>
      <Btn title="Insert column left" onClick={() => run(view, addColumnBefore)}>
        <BetweenVerticalStart />
      </Btn>
      <Btn title="Insert column right" onClick={() => run(view, addColumnAfter)}>
        <BetweenVerticalEnd />
      </Btn>
      <span className="md-float-sep" />
      <Btn title="Align column left" onClick={align('left')}>
        <AlignLeft />
      </Btn>
      <Btn title="Center column" onClick={align('center')}>
        <AlignCenter />
      </Btn>
      <Btn title="Align column right" onClick={align('right')}>
        <AlignRight />
      </Btn>
      <span className="md-float-sep" />
      <Btn title="Delete row" onClick={() => run(view, deleteRow)}>
        <Minus />
      </Btn>
      <Btn title="Delete column" onClick={() => run(view, deleteColumn)}>
        <Minus style={{ transform: 'rotate(90deg)' }} />
      </Btn>
      <Btn title="Delete table" danger onClick={() => run(view, deleteTable)}>
        <Trash2 />
      </Btn>
    </Floating>
  );
}

// ── slash menu ─────────────────────────────────────────────────────────────────────────────────

export interface SlashItem {
  id: string;
  label: string;
  icon: ReactNode;
  hint?: string;
  keywords: string;
  run(view: EditorView): void;
}

export function slashItems(actions: { pickLink(): void; webLink(): void }): SlashItem[] {
  const cmd = (c: (s: EditorState, d?: EditorView['dispatch']) => boolean) => (view: EditorView) => run(view, c);
  return [
    { id: 'p', label: 'Text', icon: <Pilcrow />, keywords: 'paragraph plain', run: cmd(setParagraph) },
    { id: 'h1', label: 'Heading 1', icon: <Heading1 />, hint: '#', keywords: 'title h1', run: cmd(toggleHeading(1)) },
    { id: 'h2', label: 'Heading 2', icon: <Heading2 />, hint: '##', keywords: 'subtitle h2', run: cmd(toggleHeading(2)) },
    { id: 'h3', label: 'Heading 3', icon: <Heading3 />, hint: '###', keywords: 'h3', run: cmd(toggleHeading(3)) },
    { id: 'ul', label: 'Bulleted list', icon: <List />, hint: '-', keywords: 'bullet unordered ul', run: cmd(toggleList(N.bulletList)) },
    { id: 'ol', label: 'Numbered list', icon: <ListOrdered />, hint: '1.', keywords: 'ordered ol', run: cmd(toggleList(N.orderedList)) },
    { id: 'task', label: 'Task list', icon: <ListTodo />, hint: '[ ]', keywords: 'todo checkbox check', run: cmd(toggleTaskList) },
    { id: 'quote', label: 'Quote', icon: <Quote />, hint: '>', keywords: 'blockquote callout', run: cmd(toggleQuote) },
    { id: 'code', label: 'Code block', icon: <SquareCode />, hint: '```', keywords: 'snippet fence pre', run: cmd(setBlockType(N.codeBlock)) },
    { id: 'table', label: 'Table', icon: <Table />, keywords: 'grid columns', run: cmd(insertTable(3, 3)) },
    { id: 'hr', label: 'Divider', icon: <Minus />, hint: '---', keywords: 'rule horizontal separator', run: cmd(insertRule) },
    { id: 'link', label: 'Link to…', icon: <Link2 />, hint: '[[', keywords: 'devall request note todo environment item', run: () => actions.pickLink() },
    { id: 'url', label: 'Web link', icon: <Globe />, keywords: 'url href http', run: () => actions.webLink() },
  ];
}

export function filterSlash(items: SlashItem[], q: string): SlashItem[] {
  const s = q.trim().toLowerCase();
  if (!s) return items;
  return items.filter((it) => it.label.toLowerCase().includes(s) || it.keywords.includes(s) || it.id === s);
}

export function SlashMenu({ view, items, hl, onHl, onPick }: { view: EditorView; items: SlashItem[]; hl: number; onHl(i: number): void; onPick(it: SlashItem): void }) {
  const st = slashKey.getState(view.state);
  const list = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    list.current?.querySelector('.hl')?.scrollIntoView({ block: 'nearest' });
  }, [hl]);
  if (!st) return null;
  const c = view.coordsAtPos(st.from);
  if (!items.length)
    return (
      <Floating x={c.left + 110} y={c.bottom} label="Insert" className="md-slash">
        <div className="md-slash-empty">No matching block — Esc to close</div>
      </Floating>
    );
  return (
    <Floating x={c.left + 110} y={c.bottom} label="Insert" className="md-slash">
      <div ref={list} className="md-slash-list" role="listbox">
        {items.map((it, i) => (
          <div key={it.id} role="option" aria-selected={i === hl} className={clsx('md-slash-item', i === hl && 'hl')} onMouseEnter={() => onHl(i)} onMouseDown={(e) => e.preventDefault()} onClick={() => onPick(it)}>
            {it.icon}
            <span className="grow">{it.label}</span>
            {it.hint && <span className="md-slash-hint">{it.hint}</span>}
          </div>
        ))}
      </div>
    </Floating>
  );
}

/** Removes the typed `/query` and runs the item. */
export function runSlash(view: EditorView, it: SlashItem) {
  const st = slashKey.getState(view.state);
  if (st) view.dispatch(view.state.tr.delete(st.from, view.state.selection.from).setMeta(slashKey, { close: true }));
  else closeSlash(view);
  it.run(view);
}
