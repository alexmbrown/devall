import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { FolderKanban, GripVertical, Hash, Inbox, MoreHorizontal } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import type { TaskView } from '@/lib/api/todos';
import { moveTasks, updateTasks, useTodos } from '@/store/todos';
import { boardColumns, boardDrop, childCounts, columnDefaults, viewTasks, type BoardColumn } from './boardModel';
import { AddTask, CheckCircle, InlineText, Meta } from './TaskRow';
import { PriorityIcon } from './pickers';
import { colorVar, dueInfo, halfOf, rangeSelect, type ViewRef } from './lib';
import { beginDrag, endDrag, useOptions, useTodoUi } from './ui';
import { AddSection, reorderLists, reorderSections, sectionMenu, useDropZone, useOrder } from './views';

/** Applies a card drop computed by `boardDrop`. */
function dropInto(col: BoardColumn, moving: string[], over: { id: string; pos: 'before' | 'after' } | null) {
  const tasks = useTodos.getState().snap?.tasks ?? [];
  const d = boardDrop(tasks, col, moving, over);
  if (!d) return;
  const n = moving.length;
  const what = n === 1 ? 'task' : `${n} tasks`;
  if (d.kind === 'move') {
    const sameColumn = moving.every((id) => col.tasks.some((t) => t.id === id));
    void moveTasks(moving, d.target, sameColumn ? undefined : `Moved ${what} to ${col.title}`);
  } else void updateTasks(moving, d.patch, `Moved ${what} to ${col.title}`);
}

function Card({ task, col, kids }: { task: TaskView; col: BoardColumn; kids?: { open: number; all: number } }) {
  const snap = useTodos((s) => s.snap);
  const isOpen = useTodos((s) => s.openId === task.id);
  const isSel = useTodoUi((s) => s.sel.includes(task.id));
  const isCursor = useTodoUi((s) => s.cursor === task.id);
  const dragging = useTodoUi((s) => !!s.drag?.includes(task.id));
  const dropHere = useTodoUi((s) => (s.drop?.id === task.id && s.drop.pos !== 'child' ? s.drop.pos : null));
  const today = snap?.today ?? '';
  const ids = () => {
    const sel = useTodoUi.getState().sel;
    return sel.includes(task.id) ? sel : [task.id];
  };
  return (
    <div
      className={clsx('td-card', task.done && 'done', isOpen && 'open-detail', isSel && 'sel', isCursor && 'cursor', dragging && 'dragging', dropHere && `drop-${dropHere}`)}
      data-task={task.id}
      tabIndex={-1}
      draggable={!task.done}
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
      <Meta t={task} info={dueInfo(task, today)} today={today} kids={kids?.all ?? 0} open={kids?.open ?? 0} where={col.target.kind === 'slot' ? undefined : snap?.lists.find((l) => l.id === task.listId)?.name} />
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

function Column({ col, kids, fallbackList }: { col: BoardColumn; kids: Map<string, { open: number; all: number }>; fallbackList: string }) {
  const [over, setOver] = useState(false);
  const droppable = col.target.kind !== 'none';
  return (
    <section className={clsx('td-column', over && 'drop')} aria-label={col.title}>
      <ColumnHead col={col} />
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
        {col.tasks.map((t) => (
          <Card key={t.id} task={t} col={col} kids={kids.get(t.id)} />
        ))}
        {droppable && <AddTask slot={`board:${col.key}`} defaults={columnDefaults(col, fallbackList)} className="td-card-add" />}
      </div>
    </section>
  );
}

/**
 * The Board layout of a view: one column per section / list / priority / day (the view's
 * "Group by"), cards dragged between columns (move, priority, date) and within ordered columns
 * (position). Columns scroll sideways inside the board only.
 */
export function BoardView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const showDone = useTodoUi((s) => s.showDone);
  const opts = useOptions(view);
  const listId = view.kind === 'list' ? view.id : view.kind === 'inbox' ? 'inbox' : undefined;
  const tasks = useMemo(() => viewTasks(snap, view, showDone), [snap, view, showDone]);
  const columns = useMemo(() => boardColumns(snap, tasks, opts.groupBy, { listId, sort: opts.sort, today: snap.today }), [snap, tasks, opts.groupBy, opts.sort, listId]);
  const kids = useMemo(() => childCounts(snap.tasks), [snap]);
  useOrder(columns.flatMap((c) => c.tasks.map((t) => t.id)));
  const fallbackList = listId ?? snap.projectList ?? 'inbox';
  return (
    <div className="td-board" aria-label="Board">
      {columns.map((c) => (
        <Column key={c.key} col={c} kids={kids} fallbackList={fallbackList} />
      ))}
      {opts.groupBy === 'section' && listId && (
        <section className="td-column td-column-new">
          <AddSection listId={listId} />
        </section>
      )}
    </div>
  );
}
