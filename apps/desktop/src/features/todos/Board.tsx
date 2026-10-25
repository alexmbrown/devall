import { useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { ArrowLeft, ArrowRight, ChevronDown, Columns3, FoldHorizontal, FolderKanban, Gauge, GripVertical, Hash, Inbox, MoreHorizontal, Palette, Pencil, Plus, Trash2, UnfoldHorizontal } from 'lucide-react';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { promptDialog } from '@/components/ui/dialogs';
import type { NewTask, StatusColumn, TaskView } from '@/lib/api/todos';
import { moveTasks, saveColumns, setStatus, updateTasks, useTodos } from '@/store/todos';
import { boardColumns, boardDrop, boardTasks, childCounts, columnDefaults, insertColumn, moveColumn, viewDefaults, type BoardColumn } from './boardModel';
import { openTodoDialog } from './dialogs';
import { AddTask, CheckCircle, InlineText, Meta } from './TaskRow';
import { ColorPicker, PriorityIcon, StatusDot } from './pickers';
import { Popover, usePopover } from './Popover';
import { colorVar, dueInfo, halfOf, rangeSelect, reorderIds, type ViewRef } from './lib';
import { beginDrag, DONE_DAYS, endDrag, useBoardPrefs, useOptions, useTodoUi } from './ui';
import { AddSection, reorderLists, reorderSections, sectionMenu, useDropZone, useOrder } from './views';

/** Done cards shown before "Show more". */
const DONE_PAGE = 20;

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

function Card({ task, col, kids, showWhere }: { task: TaskView; col: BoardColumn; kids?: { open: number; all: number }; showWhere: boolean }) {
  const snap = useTodos((s) => s.snap);
  const isOpen = useTodos((s) => s.openId === task.id);
  const isSel = useTodoUi((s) => s.sel.includes(task.id));
  const isCursor = useTodoUi((s) => s.cursor === task.id);
  const dragging = useTodoUi((s) => !!s.drag?.includes(task.id));
  const dropHere = useTodoUi((s) => (s.drop?.id === task.id && s.drop.pos !== 'child' ? s.drop.pos : null));
  const today = snap?.today ?? '';
  const status = col.target.kind === 'status';
  const ids = () => {
    const sel = useTodoUi.getState().sel;
    return sel.includes(task.id) ? sel : [task.id];
  };
  return (
    <div
      className={clsx('td-card', task.done && 'done', isOpen && 'open-detail', isSel && 'sel', isCursor && 'cursor', dragging && 'dragging', dropHere && `drop-${dropHere}`)}
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
      <div className="td-card-top">
        <CheckCircle task={task} />
        <span className="td-content">
          <InlineText text={task.content} />
        </span>
      </div>
      {task.description?.trim() && <div className="td-card-desc">{task.description.trim().split('\n')[0]}</div>}
      <Meta
        t={task}
        info={dueInfo(task, today)}
        today={today}
        kids={kids?.all ?? 0}
        open={kids?.open ?? 0}
        hideStatus={status}
        where={showWhere && col.target.kind !== 'slot' ? snap?.lists.find((l) => l.id === task.listId)?.name : undefined}
      />
    </div>
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

/** Header of a status column: colour, name (double-click renames), count or `n/limit`, actions; drag to reorder. */
function StatusColumnHead({ col, column }: { col: BoardColumn; column: StatusColumn }) {
  const cols = useTodos((s) => s.snap?.settings.columns) ?? [];
  const doneDays = useBoardPrefs((s) => s.doneDays);
  const setBoard = useBoardPrefs((s) => s.set);
  const [renaming, setRenaming] = useState(false);
  const color = usePopover();
  const head = useRef<HTMLDivElement>(null);
  const zone = useDropZone((ids) => dropInto(col, ids, null), { kind: 'column', axis: 'x', onItem: (moving, pos) => reorderColumns(moving, column.id, pos) });
  const n = column.done ? col.tasks.length : col.tasks.filter((t) => !t.done).length;
  const limit = column.wipLimit ?? 0;
  const over = limit > 0 && n > limit;
  const i = cols.findIndex((c) => c.id === column.id);
  const openColumns = cols.filter((c) => !c.done).length;
  const items: MenuItemDef[] = [
    { label: 'Rename', icon: <Pencil />, onSelect: () => setRenaming(true) },
    { label: 'Colour…', icon: <Palette />, onSelect: () => head.current && color.openAt(head.current) },
    ...(column.done ? [] : [{ label: limit ? `WIP limit (${limit})…` : 'WIP limit…', icon: <Gauge />, onSelect: () => void editLimit(column) }]),
    { label: 'Move left', icon: <ArrowLeft />, disabled: i <= 0, onSelect: () => void saveColumns(moveColumn(cols, column.id, -1)) },
    { label: 'Move right', icon: <ArrowRight />, disabled: i < 0 || i >= cols.length - 1, onSelect: () => void saveColumns(moveColumn(cols, column.id, 1)) },
    ...(column.done
      ? [
          { separator: true },
          { heading: 'Show completed' },
          ...DONE_DAYS.map((d) => ({ label: d === 1 ? 'Today' : `Last ${d} days`, hint: doneDays === d ? '✓' : undefined, onSelect: () => setBoard({ doneDays: d }) })),
          { label: 'Collapse column', icon: <FoldHorizontal />, onSelect: () => setBoard({ doneCollapsed: true }) },
        ]
      : []),
    { separator: true },
    { label: 'Edit columns…', icon: <Columns3 />, onSelect: () => openTodoDialog({ kind: 'columns' }) },
    ...(column.done ? [] : [{ label: 'Delete column…', icon: <Trash2 />, danger: true, disabled: openColumns <= 1, onSelect: () => openTodoDialog({ kind: 'deleteColumn', id: column.id }) }]),
  ];
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
          <span className="truncate strong" onDoubleClick={() => setRenaming(true)} title={`${column.name}${column.done ? ' — moving a task here completes it' : ''}`}>
            {column.name}
          </span>
        )}
        <span className={clsx('count', over && 'over')} title={limit ? `${n} of ${limit} (WIP limit)${over ? ' — over the limit' : ''}` : undefined}>
          {limit ? `${n}/${limit}` : n || ''}
        </span>
        <span className="spacer" />
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
  kids,
  defaults,
  showWhere,
  doneAll,
  onDoneAll,
}: {
  col: BoardColumn;
  kids: Map<string, { open: number; all: number }>;
  defaults: Omit<NewTask, 'text'>;
  showWhere: boolean;
  doneAll: boolean;
  onDoneAll(): void;
}) {
  const [over, setOver] = useState(false);
  const droppable = col.target.kind !== 'none';
  const done = col.target.kind === 'status' && col.target.done;
  // A long done column shows the latest completions first, the rest on demand.
  const shown = done && !doneAll ? col.tasks.slice(0, DONE_PAGE) : col.tasks;
  return (
    <section className={clsx('td-column', over && 'drop', done && 'done')} aria-label={col.title}>
      {col.column ? <StatusColumnHead col={col} column={col.column} /> : <ColumnHead col={col} />}
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
        {shown.map((t) => (
          <Card key={t.id} task={t} col={col} kids={kids.get(t.id)} showWhere={showWhere} />
        ))}
        {shown.length < col.tasks.length && (
          <button className="td-add-btn td-card-add" onClick={onDoneAll}>
            <ChevronDown /> Show {col.tasks.length - shown.length} more
          </button>
        )}
        {done && !col.tasks.length && <div className="td-column-hint">Drop a card here to complete it.</div>}
        {droppable && !done && <AddTask slot={`board:${col.key}`} defaults={columnDefaults(col, defaults)} className="td-card-add" />}
      </div>
    </section>
  );
}

/** The done column folded to a strip (still a drop target: dropping completes). */
function CollapsedColumn({ col }: { col: BoardColumn }) {
  const setBoard = useBoardPrefs((s) => s.set);
  const zone = useDropZone((ids) => dropInto(col, ids, null));
  return (
    <section className={clsx('td-column collapsed', zone.over && 'drop')} aria-label={col.title} {...zone.props}>
      <button className="td-column-expand" onClick={() => setBoard({ doneCollapsed: false })} title={`Show the ${col.title} column`} aria-label={`Expand ${col.title}`}>
        <UnfoldHorizontal />
        <StatusDot col={col.column} />
        <span className="td-column-vtitle">{col.title}</span>
        <span className="count">{col.tasks.length || ''}</span>
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
 * ordered columns (position). Only the column strip scrolls sideways.
 */
export function BoardView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const showDone = useTodoUi((s) => s.showDone);
  const doneDays = useBoardPrefs((s) => s.doneDays);
  const doneCollapsed = useBoardPrefs((s) => s.doneCollapsed);
  const [doneAll, setDoneAll] = useState(false);
  const opts = useOptions(view);
  const listId = view.kind === 'list' ? view.id : view.kind === 'inbox' ? 'inbox' : undefined;
  const tasks = useMemo(() => boardTasks(snap, view, opts.groupBy, showDone, doneDays), [snap, view, opts.groupBy, showDone, doneDays]);
  const columns = useMemo(() => boardColumns(snap, tasks, opts.groupBy, { listId, sort: opts.sort, today: snap.today }), [snap, tasks, opts.groupBy, opts.sort, listId]);
  const kids = useMemo(() => childCounts(snap.tasks), [snap]);
  const defaults = useMemo(() => viewDefaults(view, snap), [view, snap]);
  const folded = (c: BoardColumn) => !!c.column?.done && doneCollapsed;
  useOrder(columns.filter((c) => !folded(c)).flatMap((c) => (c.column?.done && !doneAll ? c.tasks.slice(0, DONE_PAGE) : c.tasks).map((t) => t.id)));
  return (
    <div className="td-board" aria-label="Board">
      {columns.map((c) =>
        folded(c) ? (
          <CollapsedColumn key={c.key} col={c} />
        ) : (
          <Column key={c.key} col={c} kids={kids} defaults={defaults} showWhere={!listId} doneAll={doneAll} onDoneAll={() => setDoneAll(true)} />
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
