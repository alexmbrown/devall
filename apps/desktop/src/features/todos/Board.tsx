import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  AlignLeft,
  ArrowLeft,
  ArrowRight,
  ArrowRightLeft,
  CalendarDays,
  Check,
  ChevronDown,
  Columns3,
  CopyPlus,
  CornerDownRight,
  FoldHorizontal,
  FolderKanban,
  Gauge,
  GripVertical,
  Hash,
  Image as ImageIcon,
  Inbox,
  Link2,
  ListChecks,
  MessageSquare,
  MoreHorizontal,
  Palette,
  PanelRightOpen,
  Paperclip,
  Pencil,
  Plus,
  Repeat,
  Tag,
  Target,
  Trash2,
  Undo2,
  UnfoldHorizontal,
} from 'lucide-react';
import { copyLinkItems } from '@/components/links/LinkChip';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { promptDialog } from '@/components/ui/dialogs';
import { formatLink } from '@/lib/links/syntax';
import type { Attachment, NewTask, StatusColumn, TaskView } from '@/lib/api/todos';
import { addTask, completeTasks, deleteTasks, moveTasks, saveColumns, setStatus, updateTasks, useTodos } from '@/store/todos';
import { previewable, useAttachmentUrl } from './attachmentUrl';
import { boardColumns, boardDrop, boardTasks, cardChecklist, childCounts, columnDefaults, columnKeyOf, doneTasksInView, foldSubtasks, inDoneWindow, insertColumn, moveColumn, statusChip, statusOf, viewDefaults, type BoardColumn } from './boardModel';
import { openTodoDialog } from './dialogs';
import { DuePicker } from './DuePicker';
import { filterCtx, isFilterActive, matchesFilter, olderDoneIn, viewLists, type Hidden } from './filterModel';
import { QuickAddInput } from './QuickAddInput';
import { AddTask, CheckCircle, Composer, InlineText } from './TaskRow';
import { ColorPicker, LabelPicker, MovePicker, PriorityIcon, priorityItems, StatusDot, statusItems } from './pickers';
import { Popover, usePopover } from './Popover';
import { boardKeyAction, boardNav, colorVar, dayLabel, dueInfo, halfOf, labelColor, rangeSelect, reorderIds, type ViewRef } from './lib';
import { beginDrag, DONE_DAYS, doneDaysLabel, endDrag, useBoardPrefs, useOptions, useTodoUi, useViewFilter } from './ui';
import { AddSection, reorderLists, reorderSections, sectionMenu, useDropZone, useOrder, useViewStats } from './views';

/** Done cards shown before "Show more". */
const DONE_PAGE = 20;

/** The board on screen (its columns), for keyboard moves between columns. */
let current: { columns: BoardColumn[] } | null = null;

/** Applies a card drop computed by `boardDrop`. */
function dropInto(col: BoardColumn, moving: string[], over: { id: string; pos: 'before' | 'after' } | null) {
  const tasks = useTodos.getState().snap?.tasks ?? [];
  const d = boardDrop(tasks, col, moving, over);
  if (!d) return;
  const n = moving.length;
  const what = n === 1 ? 'task' : `${n} tasks`;
  const sameColumn = moving.every((id) => col.tasks.some((t) => t.id === id));
  if (d.kind === 'status') {
    const done = col.target.kind === 'status' && col.target.done;
    void setStatus(moving, d.status, { before: d.before, after: d.after }, sameColumn ? undefined : done ? (n === 1 ? 'Completed' : `Completed ${n} tasks`) : `Moved ${what} to ${col.title}`);
  } else if (d.kind === 'move') void moveTasks(moving, d.target, sameColumn ? undefined : `Moved ${what} to ${col.title}`);
  else void updateTasks(moving, d.patch, `Moved ${what} to ${col.title}`);
}

const currentColumns = () => useTodos.getState().snap?.settings.columns ?? [];

/** Saves the columns with `id` changed by `patch`. */
function patchColumn(id: string, patch: Partial<StatusColumn>) {
  void saveColumns(currentColumns().map((c) => (c.id === id ? { ...c, ...patch } : c)));
}

/** Reorders status columns: `moving` before/after `target` (dragging a header). */
function reorderColumns(moving: string, target: string, pos: 'before' | 'after') {
  const cols = currentColumns();
  const ids = cols.map((c) => c.id);
  const next = reorderIds(ids, moving, target, pos);
  if (next.join() !== ids.join()) void saveColumns(next.map((id) => cols.find((c) => c.id === id)!));
}

async function editLimit(column: StatusColumn) {
  const v = await promptDialog({
    title: `WIP limit for “${column.name}”`,
    label: 'Most open cards in this column (empty: no limit)',
    initial: column.wipLimit ? String(column.wipLimit) : '',
    placeholder: 'No limit',
    confirmText: 'Save',
    validate: (s) => (s.trim() && !/^\d{1,4}$/.test(s.trim()) ? 'A whole number' : undefined),
  });
  if (v !== null) patchColumn(column.id, { wipLimit: Number(v.trim()) || null });
}

/** Folds a column to a strip (the done column keeps its own switch) or unfolds it. */
function setCollapsed(col: BoardColumn, on: boolean) {
  const b = useBoardPrefs.getState();
  if (col.column?.done) b.set({ doneCollapsed: on });
  else b.set({ collapsed: on ? [...new Set([...b.collapsed, col.key])] : b.collapsed.filter((k) => k !== col.key) });
}

/** A copy of the card right under it (same list, column, labels, dates…). */
async function copyCard(t: TaskView) {
  const id = await addTask(
    { text: t.content, parse: false, listId: t.listId, sectionId: t.sectionId, parentId: t.parentId, afterId: t.id, description: t.description, priority: t.priority, due: t.due, deadline: t.deadline, labels: t.labels, links: t.links, status: t.done ? undefined : t.status },
    true,
  );
  if (id && !t.done) {
    const col = statusOf(t, currentColumns());
    if (col) await setStatus([id], col.id, { after: t.id }, 'Copied the card');
  }
}

function focusCard(id: string | null) {
  if (!id) return;
  const el = document.querySelector<HTMLElement>(`.td-root .td-card[data-task="${CSS.escape(id)}"]`);
  el?.focus({ preventScroll: true });
  el?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}

/**
 * Board keyboard (Trello-like): arrows move between cards, Enter opens, `n` new card in the column,
 * `e` quick edit, `l` labels, `d` due date, `c` complete, Space selects, `<` `>` move to the
 * previous/next column. Returns whether the key was the board's.
 */
export function handleBoardKey(e: React.KeyboardEvent): boolean {
  const a = boardKeyAction(e);
  const ui = useTodoUi.getState();
  const grid = ui.grid;
  if (!a || !grid) return false;
  const cur = ui.cursor && grid.ids.some((c) => c.includes(ui.cursor!)) ? ui.cursor : null;
  const targets = ui.sel.length ? ui.sel : cur ? [cur] : [];
  const st = useTodos.getState();
  e.preventDefault();
  e.stopPropagation();
  const colOf = (id: string | null) => (id ? grid.ids.findIndex((c) => c.includes(id)) : -1);
  switch (a) {
    case 'up':
    case 'down':
    case 'left':
    case 'right': {
      const next = boardNav(grid.ids, cur, a);
      ui.set({ cursor: next, anchor: next });
      focusCard(next);
      break;
    }
    case 'open':
      if (cur) st.open(cur);
      break;
    case 'newCard': {
      // The cursor's column, else the first one that takes new cards.
      const cols = current?.columns ?? [];
      const takes = (k: string) => {
        const c = cols.find((x) => x.key === k);
        return !!c && c.target.kind !== 'none' && !(c.target.kind === 'status' && c.target.done);
      };
      const here = grid.keys[colOf(cur)];
      const key = here && takes(here) ? here : grid.keys.find(takes);
      if (key) {
        ui.set({ composer: `board:${key}` });
        setTimeout(() => document.querySelector(`.td-column[data-col="${CSS.escape(key)}"] .td-composer`)?.scrollIntoView({ block: 'nearest' }), 0);
      }
      break;
    }
    case 'quickEdit':
      if (cur) ui.set({ quickEdit: cur });
      break;
    case 'labels':
    case 'due':
      if (cur) ui.set({ cardPicker: { id: cur, kind: a } });
      break;
    case 'complete':
      if (targets.length) void completeTasks(targets, !st.snap?.tasks.find((t) => t.id === targets[0])?.done);
      break;
    case 'toggleSelect':
      if (cur) ui.set({ sel: ui.sel.includes(cur) ? ui.sel.filter((x) => x !== cur) : [...ui.sel, cur], anchor: cur });
      break;
    case 'moveLeft':
    case 'moveRight': {
      const cols = current?.columns ?? [];
      const at = cols.findIndex((c) => c.key === grid.keys[colOf(cur)]);
      if (at < 0 || !targets.length) break;
      const step = a === 'moveLeft' ? -1 : 1;
      for (let j = at + step; j >= 0 && j < cols.length; j += step)
        if (cols[j].target.kind !== 'none') {
          dropInto(cols[j], targets, null);
          setTimeout(() => focusCard(cur), 50);
          break;
        }
      break;
    }
  }
  return true;
}

// ── cards ─────────────────────────────────────────────────────────────────────────────────────

function CoverImage({ taskId, a }: { taskId: string; a: Attachment }) {
  const { url } = useAttachmentUrl(taskId, a);
  return <div className="td-card-cover image">{url && <img src={url} alt="" draggable={false} />}</div>;
}

/** The card's cover: its chosen image attachment or a colour band. */
function CardCover({ task }: { task: TaskView }) {
  const c = task.cover;
  const img = c?.attachmentId ? task.attachments?.find((x) => x.id === c.attachmentId && previewable(x)) : undefined;
  if (img) return <CoverImage taskId={task.id} a={img} />;
  const color = colorVar(c?.color);
  return color ? <div className="td-card-cover color" style={{ background: color }} /> : null;
}

function CoverThumb({ taskId, a, on, onPick }: { taskId: string; a: Attachment; on: boolean; onPick(): void }) {
  const { url } = useAttachmentUrl(taskId, a);
  return (
    <button className={clsx('td-cover-thumb', on && 'on')} onClick={onPick} title={a.name} aria-label={`Use ${a.name} as the cover`}>
      {url ? <img src={url} alt="" draggable={false} /> : <ImageIcon />}
    </button>
  );
}

/** Picks the card's cover: a colour, or one of its image attachments. */
function CoverPicker({ task, anchor, onClose }: { task: TaskView; anchor: DOMRect; onClose(): void }) {
  const images = (task.attachments ?? []).filter(previewable);
  const cover = task.cover;
  return (
    <Popover anchor={anchor} onClose={onClose} className="wide">
      <div className="td-pop-title">Cover colour</div>
      <ColorPicker value={cover?.color ?? null} onChange={(c) => void updateTasks([task.id], { cover: c ? { color: c } : null })} custom noneLabel="No cover" />
      <div className="td-pop-title">Image</div>
      {images.length ? (
        <div className="td-cover-thumbs">
          {images.map((a) => (
            <CoverThumb key={a.id} taskId={task.id} a={a} on={cover?.attachmentId === a.id} onPick={() => void updateTasks([task.id], { cover: { attachmentId: a.id } })} />
          ))}
        </div>
      ) : (
        <div className="subtle xs">Attach an image to the task (drop or paste it on the open card) to use it as the cover.</div>
      )}
      {cover && (
        <button
          className="btn btn-sm btn-ghost td-pop-wide-btn"
          onClick={() => {
            void updateTasks([task.id], { cover: null });
            onClose();
          }}
        >
          Remove cover
        </button>
      )}
    </Popover>
  );
}

/** Trello-style badges: due (coloured overdue / today / done), deadline, checklist "2/5", description, comments, attachments, links. */
function CardBadges({ task, kids, where, hideStatus, today }: { task: TaskView; kids?: { open: number; all: number }; where?: string; hideStatus: boolean; today: string }) {
  const columns = useTodos((s) => s.snap?.settings.columns);
  const info = dueInfo(task, today);
  const status = !hideStatus && columns ? statusChip(task, columns) : undefined;
  const list = cardChecklist(task, kids);
  const items: React.ReactNode[] = [];
  if (status)
    items.push(
      <span key="status" className="td-status" style={{ ['--c' as string]: colorVar(status.color) }} title={`Status: ${status.name}`}>
        <span className="truncate">{status.name}</span>
      </span>,
    );
  if (info) {
    const tone = task.done ? 'complete' : info.late || info.tone === 'overdue' ? 'overdue' : info.tone === 'today' ? 'today' : info.tone === 'tomorrow' ? 'soon' : '';
    items.push(
      <span key="due" className={clsx('td-badge td-badge-due', tone)} title={`${task.done ? 'Completed · ' : tone === 'overdue' ? 'Overdue · ' : ''}Due ${info.text}${info.time ? ` ${info.time}` : ''}`}>
        {info.recurring ? <Repeat /> : <CalendarDays />}
        <span className="truncate">
          {info.text}
          {info.time ? ` ${info.time}` : ''}
        </span>
      </span>,
    );
  }
  if (task.deadline)
    items.push(
      <span key="dl" className={clsx('td-badge', task.deadline < today && !task.done && 'overdue')} title="Deadline">
        <Target />
        <span className="truncate">{dayLabel(task.deadline, today)}</span>
      </span>,
    );
  if (list.total)
    items.push(
      <span key="cl" className={clsx('td-badge', list.done === list.total && 'complete')} title="Checklist: sub-tasks and task-list items in the description">
        <ListChecks />
        {list.done}/{list.total}
      </span>,
    );
  if (task.description?.trim())
    items.push(
      <span key="d" className="td-badge" title="Has a description">
        <AlignLeft />
      </span>,
    );
  if (task.comments?.length)
    items.push(
      <span key="c" className="td-badge" title="Comments">
        <MessageSquare />
        {task.comments.length}
      </span>,
    );
  if (task.attachments?.length)
    items.push(
      <span key="a" className="td-badge" title={task.attachments.map((a) => a.name).join('\n')}>
        <Paperclip />
        {task.attachments.length}
      </span>,
    );
  if (task.links?.length)
    items.push(
      <span key="l" className="td-badge" title={task.links.join('\n')}>
        <Link2 />
        {task.links.length}
      </span>,
    );
  if (where)
    items.push(
      <span key="w" className="td-listname" title={where}>
        <span className="truncate">{where}</span>
      </span>,
    );
  return items.length ? <div className="td-meta td-card-badges">{items}</div> : null;
}

type CardPicker = 'labels' | 'due' | 'move' | 'cover';

/** Card actions: the "…" menu, the context menu and the quick editor share them. */
function cardMenu(task: TaskView, ids: () => string[], openPicker: (p: CardPicker) => void): MenuItemDef[] {
  const snap = useTodos.getState().snap;
  const cols = snap?.settings.columns ?? [];
  const n = ids().length;
  return [
    { label: 'Open card', icon: <PanelRightOpen />, hint: 'Enter', onSelect: () => useTodos.getState().open(task.id) },
    { label: 'Quick edit', icon: <Pencil />, hint: 'E', onSelect: () => useTodoUi.getState().set({ quickEdit: task.id }) },
    { separator: true },
    { heading: n > 1 ? `Move ${n} cards to column` : 'Move to column' },
    ...statusItems(cols, statusOf(task, cols)?.id, (c) => void setStatus(ids(), c.id, undefined, c.done ? (n === 1 ? 'Completed' : `Completed ${n} tasks`) : `Moved to ${c.name}`)),
    { label: 'Move to list…', icon: <ArrowRightLeft />, onSelect: () => openPicker('move') },
    { separator: true },
    { label: 'Labels…', icon: <Tag />, hint: 'L', onSelect: () => openPicker('labels') },
    { label: 'Due date…', icon: <CalendarDays />, hint: 'D', onSelect: () => openPicker('due') },
    { label: 'Cover…', icon: <ImageIcon />, onSelect: () => openPicker('cover') },
    { heading: 'Priority' },
    ...priorityItems(task.priority, (p) => void updateTasks(ids(), { priority: p })),
    { separator: true },
    { label: 'Copy card', icon: <CopyPlus />, onSelect: () => void copyCard(task) },
    { label: task.done ? 'Reopen' : n > 1 ? `Complete ${n} cards` : 'Complete', icon: task.done ? <Undo2 /> : <Check />, hint: 'C', onSelect: () => void completeTasks(ids(), !task.done) },
    ...copyLinkItems(formatLink('todo', task.id), task.content),
    { separator: true },
    { label: n > 1 ? `Delete ${n} cards` : 'Delete', icon: <Trash2 />, danger: true, hint: 'Del', onSelect: () => void deleteTasks(ids()) },
  ];
}

/**
 * Quick edit (Trello's pencil, `e`): the title in place (quick-add syntax: `tomorrow p1 @label`)
 * plus the card's actions, over the card.
 */
function QuickEdit({ task, card, ids, onClose }: { task: TaskView; card: DOMRect; ids: () => string[]; onClose(): void }) {
  const snap = useTodos((s) => s.snap);
  const [text, setText] = useState(task.content);
  const [ignore, setIgnore] = useState<string[]>([]);
  const [sub, setSub] = useState<{ kind: CardPicker; anchor: DOMRect } | null>(null);
  const done = useRef(false);
  const cols = snap?.settings.columns ?? [];
  const today = snap?.today ?? '';
  const save = async () => {
    if (done.current) return;
    done.current = true;
    if (text.trim() && text.trim() !== task.content) await useTodos.getState().apply({ type: 'edit', id: task.id, text, ignore });
    onClose();
  };
  const open = (kind: CardPicker) => (e: React.MouseEvent) => setSub({ kind, anchor: (e.currentTarget as HTMLElement).getBoundingClientRect() });
  const then = (f: () => void) => () => {
    f();
    onClose();
  };
  // Over the card: the panel's top edge on the card's.
  const anchor = useMemo(() => new DOMRect(card.left, card.top - 4, card.width, 0), [card.left, card.top, card.width]);
  return (
    <Popover anchor={anchor} onClose={onClose} className="td-quick-edit">
      <QuickAddInput value={text} onChange={setText} ignore={ignore} onIgnore={setIgnore} listId={task.listId} autoFocus onSubmit={() => void save()} onCancel={onClose} />
      <div className="td-qe-actions">
        <button className="btn btn-sm btn-ghost" onClick={then(() => useTodos.getState().open(task.id))}>
          <PanelRightOpen /> Open card
        </button>
        <button className="btn btn-sm btn-ghost" onClick={open('labels')}>
          <Tag /> Labels
        </button>
        <button className="btn btn-sm btn-ghost" onClick={open('due')}>
          <CalendarDays /> Dates
        </button>
        <Menu
          items={() => priorityItems(task.priority, (p) => void updateTasks(ids(), { priority: p }))}
          trigger={(p) => (
            <button className="btn btn-sm btn-ghost" {...p}>
              <PriorityIcon p={task.priority} /> Priority
            </button>
          )}
        />
        <Menu
          items={() => statusItems(cols, statusOf(task, cols)?.id, (c) => void setStatus(ids(), c.id, undefined, c.done ? 'Completed' : `Moved to ${c.name}`))}
          trigger={(p) => (
            <button className="btn btn-sm btn-ghost" {...p}>
              <Columns3 /> Column
            </button>
          )}
        />
        <button className="btn btn-sm btn-ghost" onClick={open('move')}>
          <ArrowRightLeft /> List
        </button>
        <button className="btn btn-sm btn-ghost" onClick={open('cover')}>
          <ImageIcon /> Cover
        </button>
        <button className="btn btn-sm btn-ghost" onClick={then(() => void copyCard(task))}>
          <CopyPlus /> Copy
        </button>
        <button className="btn btn-sm btn-ghost" onClick={then(() => void completeTasks(ids(), !task.done))}>
          {task.done ? <Undo2 /> : <Check />} {task.done ? 'Reopen' : 'Complete'}
        </button>
        <button className="btn btn-sm btn-danger-ghost" onClick={then(() => void deleteTasks(ids()))}>
          <Trash2 /> Delete
        </button>
      </div>
      <div className="td-qe-foot">
        <span className="subtle xs truncate grow">Enter saves · Esc cancels</span>
        <button className="btn btn-sm btn-primary" onClick={() => void save()}>
          Save
        </button>
      </div>
      {sub?.kind === 'labels' && <LabelPicker anchor={sub.anchor} onClose={() => setSub(null)} selected={task.labels ?? []} onToggle={(name, on) => void updateTasks(ids(), on ? { addLabels: [name] } : { removeLabels: [name] })} />}
      {sub?.kind === 'due' && <DuePicker anchor={sub.anchor} onClose={() => setSub(null)} mode="due" due={task.due} today={today} onPatch={(p) => void updateTasks(ids(), p)} />}
      {sub?.kind === 'move' && (
        <MovePicker anchor={sub.anchor} onClose={() => setSub(null)} current={{ listId: task.listId, sectionId: task.sectionId }} onPick={(listId, sectionId) => void moveTasks(ids(), { listId, sectionId, parentId: null, index: null }, `Moved to ${snap?.lists.find((l) => l.id === listId)?.name ?? 'list'}`)} />
      )}
      {sub?.kind === 'cover' && <CoverPicker task={task} anchor={sub.anchor} onClose={() => setSub(null)} />}
    </Popover>
  );
}

function Card({ task, col, kids, showWhere, parentTitle }: { task: TaskView; col: BoardColumn; kids?: { open: number; all: number }; showWhere: boolean; parentTitle?: string }) {
  const snap = useTodos((s) => s.snap);
  const isOpen = useTodos((s) => s.openId === task.id);
  const isSel = useTodoUi((s) => s.sel.includes(task.id));
  const isCursor = useTodoUi((s) => s.cursor === task.id);
  const dragging = useTodoUi((s) => !!s.drag?.includes(task.id));
  const dropHere = useTodoUi((s) => (s.drop?.id === task.id && s.drop.pos !== 'child' ? s.drop.pos : null));
  const quick = useTodoUi((s) => s.quickEdit === task.id);
  const kbPicker = useTodoUi((s) => (s.cardPicker?.id === task.id ? s.cardPicker.kind : null));
  const expanded = useBoardPrefs((s) => s.labelsExpanded);
  const [picker, setPicker] = useState<CardPicker | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const ctx = useContextMenu();
  const today = snap?.today ?? '';
  const status = col.target.kind === 'status';
  const ids = () => {
    const sel = useTodoUi.getState().sel;
    return sel.includes(task.id) ? sel : [task.id];
  };
  const shown = picker ?? kbPicker;
  const closePicker = () => {
    setPicker(null);
    if (useTodoUi.getState().cardPicker) useTodoUi.getState().set({ cardPicker: null });
  };
  const rect = () => ref.current?.getBoundingClientRect() ?? new DOMRect();
  const items = () => cardMenu(task, ids, setPicker);
  const labels = task.labels ?? [];
  return (
    <>
      <div
        ref={ref}
        className={clsx('td-card', task.done && 'done', task.cover && 'has-cover', isOpen && 'open-detail', isSel && 'sel', isCursor && 'cursor', dragging && 'dragging', dropHere && `drop-${dropHere}`)}
        data-task={task.id}
        tabIndex={-1}
        // Completed cards leave the done column of a status board (reopening them).
        draggable={!task.done || status}
        onDragStart={(e) => beginDrag(e, { tasks: ids() })}
        onDragEnd={endDrag}
        onDragOver={(e) => {
          const ui = useTodoUi.getState();
          // Positions only matter in ordered columns; elsewhere the column takes the drop.
          if (!ui.drag || !col.ordered || ui.drag.includes(task.id)) return;
          const pos = halfOf(e.currentTarget.getBoundingClientRect(), e.clientY);
          if (ui.drop?.id !== task.id || ui.drop.pos !== pos) ui.set({ drop: { id: task.id, pos } });
        }}
        onContextMenu={(e) => {
          const ui = useTodoUi.getState();
          if (!ui.sel.includes(task.id)) ui.set({ sel: [], cursor: task.id, anchor: task.id });
          ctx.open(e, items());
        }}
        onClick={(e) => {
          if ((e.target as HTMLElement).closest('button, a, input')) return;
          const ui = useTodoUi.getState();
          if (e.ctrlKey || e.metaKey) ui.set({ sel: ui.sel.includes(task.id) ? ui.sel.filter((x) => x !== task.id) : [...ui.sel, task.id], cursor: task.id, anchor: task.id });
          else if (e.shiftKey) ui.set({ sel: rangeSelect(ui.order, ui.anchor ?? ui.cursor, task.id), cursor: task.id });
          else {
            ui.set({ sel: [], cursor: task.id, anchor: task.id });
            useTodos.getState().open(task.id);
          }
        }}
      >
        <CardCover task={task} />
        {labels.length > 0 && (
          <button
            className={clsx('td-card-labels', expanded && 'expanded')}
            onClick={(e) => {
              e.stopPropagation();
              useBoardPrefs.getState().set({ labelsExpanded: !expanded });
            }}
            title={expanded ? 'Hide label names' : `${labels.map((l) => `@${l}`).join(' ')} — click to show names`}
            aria-label={expanded ? 'Hide label names' : 'Show label names'}
          >
            {labels.map((l) => (
              <span key={l} className="td-lstrip" style={{ ['--c' as string]: colorVar(labelColor(l, snap?.labels)) }}>
                {expanded && <span className="truncate">{l}</span>}
              </span>
            ))}
          </button>
        )}
        <div className="td-card-top">
          <CheckCircle task={task} />
          <span className="td-content">
            <InlineText text={task.content} />
          </span>
        </div>
        {parentTitle && (
          <div className="td-card-parent" title={`Sub-task of “${parentTitle}”`}>
            <CornerDownRight />
            <span className="truncate">{parentTitle}</span>
          </div>
        )}
        <CardBadges task={task} kids={kids} today={today} hideStatus={status} where={showWhere && col.target.kind !== 'slot' ? snap?.lists.find((l) => l.id === task.listId)?.name : undefined} />
        <div className="td-card-actions">
          <button
            className="icon-btn sm"
            onClick={(e) => {
              e.stopPropagation();
              useTodoUi.getState().set({ quickEdit: task.id, cursor: task.id });
            }}
            title="Quick edit (E)"
            aria-label="Quick edit"
          >
            <Pencil />
          </button>
          <Menu
            align="right"
            items={items}
            trigger={(p) => (
              <button className="icon-btn sm" {...p} title="Card actions" aria-label="Card actions">
                <MoreHorizontal />
              </button>
            )}
          />
        </div>
      </div>
      {ctx.node}
      {quick && ref.current && <QuickEdit task={task} card={ref.current.getBoundingClientRect()} ids={ids} onClose={() => useTodoUi.getState().set({ quickEdit: null })} />}
      {shown === 'labels' && <LabelPicker anchor={rect()} onClose={closePicker} selected={task.labels ?? []} onToggle={(name, on) => void updateTasks(ids(), on ? { addLabels: [name] } : { removeLabels: [name] })} />}
      {shown === 'due' && <DuePicker anchor={rect()} onClose={closePicker} mode="due" due={task.due} today={today} onPatch={(p) => void updateTasks(ids(), p)} />}
      {shown === 'move' && (
        <MovePicker anchor={rect()} onClose={closePicker} current={{ listId: task.listId, sectionId: task.sectionId }} onPick={(listId, sectionId) => void moveTasks(ids(), { listId, sectionId, parentId: null, index: null }, `Moved to ${snap?.lists.find((l) => l.id === listId)?.name ?? 'list'}`)} />
      )}
      {shown === 'cover' && <CoverPicker task={task} anchor={rect()} onClose={closePicker} />}
    </>
  );
}

// ── columns ───────────────────────────────────────────────────────────────────────────────────

/** "+" (a card at the top) and fold buttons of a column header. */
function HeadButtons({ col, addable }: { col: BoardColumn; addable: boolean }) {
  return (
    <>
      {addable && (
        <button className="icon-btn sm" onClick={() => useTodoUi.getState().set({ composer: `board-top:${col.key}` })} title="Add a card at the top" aria-label="Add a card at the top">
          <Plus />
        </button>
      )}
      <button className="icon-btn sm" onClick={() => setCollapsed(col, true)} title="Collapse the column" aria-label="Collapse the column">
        <FoldHorizontal />
      </button>
    </>
  );
}

function ColumnHead({ col }: { col: BoardColumn }) {
  const list = col.list;
  const section = col.section;
  // Sections reorder among themselves; lists only among the user's own (Inbox and project lists stay put).
  const item = section ? { kind: 'section' as const, id: section.id } : list?.kind === 'user' ? { kind: 'list' as const, id: list.id } : undefined;
  const zone = useDropZone(col.target.kind !== 'none' ? (ids) => dropInto(col, ids, null) : undefined, item && { kind: item.kind, axis: 'x', onItem: (moving, pos) => (section ? reorderSections(section.listId, moving, section.id, pos) : reorderLists(moving, item.id, pos)) });
  const menu = section ? sectionMenu(section, col.tasks.length) : null;
  const icon = list ? list.kind === 'inbox' ? <Inbox /> : list.kind === 'project' ? <FolderKanban /> : <Hash /> : col.priority ? <PriorityIcon p={col.priority} /> : null;
  return (
    <div
      className={clsx('td-column-head', col.tone, zone.over && `drop-${zone.over}`, item && 'draggable')}
      {...zone.props}
      draggable={!!item}
      onDragStart={item ? (e) => beginDrag(e, { item }) : undefined}
      onDragEnd={item ? endDrag : undefined}
    >
      {item && (
        <span className="td-grip" aria-hidden>
          <GripVertical />
        </span>
      )}
      {icon && (
        <span className="td-group-icon" style={{ color: colorVar(list?.color) }}>
          {icon}
        </span>
      )}
      <span className="truncate strong" onDoubleClick={menu?.rename} title={col.title}>
        {col.title}
      </span>
      <span className="count">{col.tasks.filter((t) => !t.done).length || ''}</span>
      <span className="spacer" />
      <HeadButtons col={col} addable={col.target.kind !== 'none'} />
      {menu && (
        <Menu
          align="right"
          items={menu.items}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} aria-label="Section actions">
              <MoreHorizontal />
            </button>
          )}
        />
      )}
    </div>
  );
}

/** Renames a column in place (Enter / blur saves, Escape cancels). */
function RenameInput({ initial, onDone }: { initial: string; onDone(name: string | null): void }) {
  const [v, setV] = useState(initial);
  const done = useRef(false);
  const finish = (save: boolean) => {
    if (done.current) return;
    done.current = true;
    onDone(save && v.trim() && v.trim() !== initial ? v.trim() : null);
  };
  return (
    <input
      className="input sm td-column-rename"
      autoFocus
      aria-label="Column name"
      value={v}
      onChange={(e) => setV(e.target.value)}
      onFocus={(e) => e.currentTarget.select()}
      onBlur={() => finish(true)}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(true);
        if (e.key === 'Escape') finish(false);
      }}
    />
  );
}

/**
 * Header of a status column: colour, name (double-click renames), count (filtered: shown cards) or
 * `n/limit` (the WIP limit counts every open card), add-at-top, fold, actions; drag to reorder.
 */
function StatusColumnHead({ col, column, open }: { col: BoardColumn; column: StatusColumn; open: number }) {
  const cols = useTodos((s) => s.snap?.settings.columns) ?? [];
  const doneDays = useBoardPrefs((s) => s.doneDays);
  const setBoard = useBoardPrefs((s) => s.set);
  const [renaming, setRenaming] = useState(false);
  const color = usePopover();
  const head = useRef<HTMLDivElement>(null);
  const zone = useDropZone((ids) => dropInto(col, ids, null), { kind: 'column', axis: 'x', onItem: (moving, pos) => reorderColumns(moving, column.id, pos) });
  const shown = column.done ? col.tasks.length : col.tasks.filter((t) => !t.done).length;
  const limit = column.wipLimit ?? 0;
  const over = limit > 0 && open > limit;
  const i = cols.findIndex((c) => c.id === column.id);
  const openColumns = cols.filter((c) => !c.done).length;
  const items: MenuItemDef[] = [
    { label: 'Rename', icon: <Pencil />, onSelect: () => setRenaming(true) },
    { label: 'Colour…', icon: <Palette />, onSelect: () => head.current && color.openAt(head.current) },
    ...(column.done ? [] : [{ label: limit ? `WIP limit (${limit})…` : 'WIP limit…', icon: <Gauge />, onSelect: () => void editLimit(column) }]),
    { label: 'Move left', icon: <ArrowLeft />, disabled: i <= 0, onSelect: () => void saveColumns(moveColumn(cols, column.id, -1)) },
    { label: 'Move right', icon: <ArrowRight />, disabled: i < 0 || i >= cols.length - 1, onSelect: () => void saveColumns(moveColumn(cols, column.id, 1)) },
    { label: 'Collapse column', icon: <FoldHorizontal />, onSelect: () => setCollapsed(col, true) },
    ...(column.done
      ? [
          { separator: true },
          { heading: 'Show completed' },
          ...DONE_DAYS.map((d) => ({ label: d === 0 ? 'All (loads older ones)' : d === 1 ? 'Today' : `Last ${d} days`, hint: doneDays === d ? '✓' : undefined, onSelect: () => setBoard({ doneDays: d }) })),
        ]
      : []),
    { separator: true },
    { label: 'Edit columns…', icon: <Columns3 />, onSelect: () => openTodoDialog({ kind: 'columns' }) },
    ...(column.done ? [] : [{ label: 'Delete column…', icon: <Trash2 />, danger: true, disabled: openColumns <= 1, onSelect: () => openTodoDialog({ kind: 'deleteColumn', id: column.id }) }]),
  ];
  const filtered = !column.done && shown !== open;
  return (
    <>
      <div
        ref={head}
        className={clsx('td-column-head draggable', zone.over && `drop-${zone.over}`)}
        {...zone.props}
        draggable={!renaming}
        onDragStart={(e) => beginDrag(e, { item: { kind: 'column', id: column.id } })}
        onDragEnd={endDrag}
      >
        <span className="td-grip" aria-hidden>
          <GripVertical />
        </span>
        <span className="td-group-icon">
          <StatusDot col={column} />
        </span>
        {renaming ? (
          <RenameInput
            initial={column.name}
            onDone={(name) => {
              setRenaming(false);
              if (name) patchColumn(column.id, { name });
            }}
          />
        ) : (
          <span className="truncate strong" onDoubleClick={() => setRenaming(true)} title={`${column.name}${column.done ? ` — moving a task here completes it · ${doneDaysLabel(doneDays)}` : ''}`}>
            {column.name}
          </span>
        )}
        <span
          className={clsx('count', over && 'over')}
          title={limit ? `${open} of ${limit} (WIP limit)${over ? ' — over the limit' : ''}${filtered ? ` · ${shown} match the filter` : ''}` : filtered ? `${shown} of ${open} match the filter` : undefined}
        >
          {limit ? `${filtered ? `${shown} · ` : ''}${open}/${limit}` : filtered ? `${shown}/${open}` : shown || ''}
        </span>
        <span className="spacer" />
        <HeadButtons col={col} addable={!column.done} />
        <Menu
          align="right"
          items={items}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} aria-label="Column actions">
              <MoreHorizontal />
            </button>
          )}
        />
      </div>
      {color.anchor && (
        <Popover anchor={color.anchor} onClose={color.close}>
          <ColorPicker
            value={column.color}
            onChange={(c) => {
              patchColumn(column.id, { color: c });
              color.close();
            }}
          />
        </Popover>
      )}
    </>
  );
}

function Column({
  col,
  open,
  kids,
  defaults,
  showWhere,
  doneAll,
  onDoneAll,
  titles,
}: {
  col: BoardColumn;
  /** Open cards in the column without the filter (WIP). */
  open: number;
  kids: Map<string, { open: number; all: number }>;
  defaults: Omit<NewTask, 'text'>;
  showWhere: boolean;
  doneAll: boolean;
  onDoneAll(): void;
  /** Task titles by id (a sub-task card names its parent). */
  titles: Map<string, string>;
}) {
  const [over, setOver] = useState(false);
  const topOpen = useTodoUi((s) => s.composer === `board-top:${col.key}`);
  const droppable = col.target.kind !== 'none';
  const done = col.target.kind === 'status' && col.target.done;
  // A long done column shows the latest completions first, the rest on demand.
  const shown = done && !doneAll ? col.tasks.slice(0, DONE_PAGE) : col.tasks;
  const colDefaults = columnDefaults(col, defaults);
  return (
    <section className={clsx('td-column', over && 'drop', done && 'done')} aria-label={col.title} data-col={col.key}>
      {col.column ? <StatusColumnHead col={col} column={col.column} open={open} /> : <ColumnHead col={col} />}
      <div
        className="td-column-body"
        onDragOver={(e) => {
          if (!droppable || !useTodoUi.getState().drag) return;
          e.preventDefault();
          if (!over) setOver(true);
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
            setOver(false);
            const ui = useTodoUi.getState();
            if (ui.drop && col.tasks.some((t) => t.id === ui.drop!.id)) ui.set({ drop: null });
          }
        }}
        onDrop={(e) => {
          const ui = useTodoUi.getState();
          setOver(false);
          if (!droppable || !ui.drag?.length) return;
          e.preventDefault();
          const moving = ui.drag;
          const at = ui.drop && ui.drop.pos !== 'child' && col.tasks.some((t) => t.id === ui.drop!.id) ? { id: ui.drop.id, pos: ui.drop.pos } : null;
          endDrag();
          dropInto(col, moving, at);
        }}
      >
        {topOpen && <Composer defaults={{ ...colDefaults, atTop: true }} onDone={() => useTodoUi.getState().set({ composer: null })} placeholder="Card title — try “tomorrow @label p1”" />}
        {shown.map((t) => (
          <Card key={t.id} task={t} col={col} kids={kids.get(t.id)} showWhere={showWhere} parentTitle={t.parentId ? titles.get(t.parentId) : undefined} />
        ))}
        {shown.length < col.tasks.length && (
          <button className="td-add-btn td-card-add" onClick={onDoneAll}>
            <ChevronDown /> Show {col.tasks.length - shown.length} more
          </button>
        )}
        {done && !col.tasks.length && <div className="td-column-hint">Drop a card here to complete it.</div>}
        {droppable && !done && <AddTask slot={`board:${col.key}`} label="Add a card" defaults={colDefaults} className="td-card-add" />}
      </div>
    </section>
  );
}

/** A column folded to a strip (still a drop target: dropping into the done column completes). */
function CollapsedColumn({ col }: { col: BoardColumn }) {
  const zone = useDropZone(col.target.kind !== 'none' ? (ids) => dropInto(col, ids, null) : undefined);
  const n = col.column?.done ? col.tasks.length : col.tasks.filter((t) => !t.done).length;
  return (
    <section className={clsx('td-column collapsed', zone.over && 'drop')} aria-label={col.title} data-col={col.key} {...zone.props}>
      <button className="td-column-expand" onClick={() => setCollapsed(col, false)} title={`Show the ${col.title} column`} aria-label={`Expand ${col.title}`}>
        <UnfoldHorizontal />
        {col.column ? <StatusDot col={col.column} /> : col.priority ? <PriorityIcon p={col.priority} /> : null}
        <span className="td-column-vtitle">{col.title}</span>
        <span className="count">{n || ''}</span>
      </button>
    </section>
  );
}

/** "+ Add column" after the status columns (goes before the done column). */
function AddColumn() {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  if (!adding)
    return (
      <button className="td-add-btn" onClick={() => setAdding(true)}>
        <Plus /> Add column
      </button>
    );
  const save = () => {
    if (name.trim()) void saveColumns(insertColumn(currentColumns(), name));
    setName('');
    setAdding(false);
  };
  return (
    <div className="row-sm td-add-section">
      <input
        className="input sm grow"
        autoFocus
        placeholder="Column name, e.g. Review"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') save();
          if (e.key === 'Escape') setAdding(false);
        }}
      />
      <button className="btn btn-sm btn-primary" disabled={!name.trim()} onClick={save}>
        Add
      </button>
      <button className="btn btn-sm btn-ghost" onClick={() => setAdding(false)}>
        Cancel
      </button>
    </div>
  );
}

/**
 * The Board layout of a view: by default one column per workflow status (To do, In progress,
 * Done — configurable), or per section / list / priority / day (the view's "Group by"). Cards are
 * dragged between columns (status — the done column completes —, move, priority, date) and within
 * ordered columns (position), several at once when selected. Trello-like cards: cover, label
 * strips, badges, quick edit, context menu; columns fold; the filter bar hides non-matching cards
 * (columns stay as drop targets). Only the column strip scrolls sideways.
 */
export function BoardView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const allDone = useTodos((s) => s.allDone);
  const showDone = useTodoUi((s) => s.showDone);
  const doneDays = useBoardPrefs((s) => s.doneDays);
  const doneCollapsed = useBoardPrefs((s) => s.doneCollapsed);
  const collapsed = useBoardPrefs((s) => s.collapsed);
  const [doneAll, setDoneAll] = useState(false);
  const opts = useOptions(view);
  const filter = useViewFilter(view);
  const listId = view.kind === 'list' ? view.id : view.kind === 'inbox' ? 'inbox' : undefined;
  const statusBoard = opts.groupBy === 'status';
  // "All completed" in the done column needs the completions older than a week.
  useEffect(() => {
    if (statusBoard && doneDays === 0 && !allDone) void useTodos.getState().setAllDone(true);
  }, [statusBoard, doneDays, allDone]);
  const tasks = useMemo(() => boardTasks(snap, view, opts.groupBy, showDone, doneDays), [snap, view, opts.groupBy, showDone, doneDays]);
  const ctx = useMemo(() => ({ listId, sort: opts.sort, today: snap.today, subtasks: opts.subtasks }), [listId, opts.sort, snap.today, opts.subtasks]);
  const active = isFilterActive(filter);
  const matching = useMemo(() => {
    if (!active) return tasks;
    const cx = filterCtx(snap, filter);
    return tasks.filter((t) => matchesFilter(t, filter, cx));
  }, [tasks, active, filter, snap]);
  const unfiltered = useMemo(() => boardColumns(snap, tasks, opts.groupBy, ctx), [snap, tasks, opts.groupBy, ctx]);
  const columns = useMemo(() => (active ? boardColumns(snap, matching, opts.groupBy, ctx) : unfiltered), [active, snap, matching, opts.groupBy, ctx, unfiltered]);
  const openIn = useMemo(() => new Map(unfiltered.map((c) => [c.key, c.tasks.filter((t) => !t.done).length])), [unfiltered]);
  const kids = useMemo(() => childCounts(snap.tasks), [snap]);
  const titles = useMemo(() => new Map(snap.tasks.map((t) => [t.id, t.content])), [snap]);
  const defaults = useMemo(() => viewDefaults(view, snap), [view, snap]);
  const folded = (c: BoardColumn) => (c.column?.done ? doneCollapsed : collapsed.includes(c.key));

  // What the board leaves out (the notices under the filter bar) and the "n of m cards".
  const nested = useMemo(() => foldSubtasks(matching, columnKeyOf(snap, opts.groupBy, ctx), opts.subtasks).folded.length, [matching, snap, opts.groupBy, ctx, opts.subtasks]);
  const older = allDone ? 0 : olderDoneIn(snap, view);
  const hidden: Hidden[] = [{ kind: 'nested', count: nested }];
  if (statusBoard) {
    if (doneDays > 0) hidden.push({ kind: 'window', count: doneTasksInView(snap, view).filter((t) => !inDoneWindow(t, snap.today, doneDays)).length + older, days: doneDays });
  } else if (viewLists(view)) {
    hidden.push(showDone ? { kind: 'older', count: older } : { kind: 'done', count: doneTasksInView(snap, view).length + older });
  }
  const cardCount = (cs: BoardColumn[]) => cs.reduce((n, c) => n + c.tasks.length, 0);
  useViewStats({ shown: cardCount(columns), total: cardCount(unfiltered), unit: 'cards', hidden });

  // Keyboard: card ids per visible column, in display order.
  const visible = columns.filter((c) => !folded(c));
  const grid = { keys: visible.map((c) => c.key), ids: visible.map((c) => (c.column?.done && !doneAll ? c.tasks.slice(0, DONE_PAGE) : c.tasks).map((t) => t.id)) };
  useOrder(grid.ids.flat());
  const gridKey = JSON.stringify(grid);
  useEffect(() => {
    useTodoUi.getState().set({ grid: JSON.parse(gridKey) as typeof grid });
  }, [gridKey]);
  useEffect(() => () => useTodoUi.getState().set({ grid: null, quickEdit: null, cardPicker: null }), []);
  useEffect(() => {
    current = { columns };
    return () => {
      current = null;
    };
  }, [columns]);

  const accent = colorVar(opts.accent);
  return (
    <div className={clsx('td-board', accent && 'accent')} style={accent ? { ['--td-board-accent' as string]: accent } : undefined} aria-label="Board">
      {columns.map((c) =>
        folded(c) ? (
          <CollapsedColumn key={c.key} col={c} />
        ) : (
          <Column key={c.key} col={c} open={openIn.get(c.key) ?? 0} kids={kids} defaults={defaults} showWhere={!listId} doneAll={doneAll} onDoneAll={() => setDoneAll(true)} titles={titles} />
        ),
      )}
      {opts.groupBy === 'section' && listId && (
        <section className="td-column td-column-new">
          <AddSection listId={listId} />
        </section>
      )}
      {opts.groupBy === 'status' && (
        <section className="td-column td-column-new">
          <AddColumn />
        </section>
      )}
    </div>
  );
}
