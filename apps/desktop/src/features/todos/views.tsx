import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { CalendarClock, CheckCircle2, ChevronDown, ChevronLeft, ChevronRight, FolderKanban, GripVertical, Hash, Inbox, MoreHorizontal, Pencil, Plus, Repeat, Rows3, Trash2, Undo2 } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { Empty, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { todosApi, type CompletedEntry, type Section, type TaskList, type TaskView } from '@/lib/api/todos';
import { completeTasks, moveTasks, updateTasks, useTodos } from '@/store/todos';
import { AddTask, InlineText, TaskRow, type RowDrop } from './TaskRow';
import { filterCtx, isFilterActive, listStats, matchesFilter, olderDoneIn, type ViewStats } from './filterModel';
import { addDays, allLayout, attention, byDue, colorVar, dayHeading, dayLabel, halfOf, listLayout, navOrder, parseYmd, reorderIds, searchTasks, startOfWeek, todayGroups, upcomingDays, weekDays, ymd, type Row, type ViewRef } from './lib';
import { beginDrag, endDrag, useOptions, useTodoUi, useViewFilter, type DragItem } from './ui';

/** Publishes the view's row order (for j/k and Shift-click). */
export function useOrder(ids: string[]) {
  const key = ids.join(',');
  useEffect(() => {
    useTodoUi.getState().set({ order: key ? key.split(',') : [] });
  }, [key]);
}

/** Publishes what the view shows and hides (the filter bar's count, the notices); cleared when it goes. */
export function useViewStats(stats: ViewStats | null) {
  const key = JSON.stringify(stats);
  useEffect(() => {
    useTodoUi.getState().set({ stats: key === 'null' ? null : (JSON.parse(key) as ViewStats) });
  }, [key]);
  useEffect(() => () => useTodoUi.getState().set({ stats: null }), []);
}

/** The view's filter as a predicate (undefined when no filter is set), memoised on the snapshot. */
export function useFilterMatch(view: ViewRef): ((t: TaskView) => boolean) | undefined {
  const snap = useTodos((s) => s.snap)!;
  const filter = useViewFilter(view);
  return useMemo(() => {
    if (!isFilterActive(filter)) return undefined;
    const cx = filterCtx(snap, filter);
    return (t: TaskView) => matchesFilter(t, filter, cx);
  }, [snap, filter]);
}

/** A flat view's tasks after the filter, with its stats published. */
function useFiltered(view: ViewRef, tasks: TaskView[]): TaskView[] {
  const match = useFilterMatch(view);
  const shown = useMemo(() => (match ? tasks.filter(match) : tasks), [tasks, match]);
  useViewStats({ shown: shown.length, total: tasks.length, unit: 'tasks', hidden: [] });
  return shown;
}

const flat = (tasks: TaskView[]): Row[] => tasks.map((task) => ({ task, depth: 0, open: 0, kids: 0 }));

function Rows({ rows, context, dueMode, drop }: { rows: Row[]; context?: 'list' | 'section'; dueMode?: 'full' | 'time'; drop?: RowDrop }) {
  return (
    <div className="td-rows">
      {rows.map((r) => (
        <TaskRow key={r.task.id} row={r} context={context} dueMode={dueMode} drop={drop} />
      ))}
    </div>
  );
}

/**
 * A drop target for dragged tasks (`onDrop(ids)`) and, optionally, for a dragged section/list
 * (`item` + `onItem(moving, pos)`: reorder before/after this one).
 */
export function useDropZone(onDrop?: (ids: string[]) => void, item?: { kind: DragItem['kind']; onItem(moving: string, pos: 'before' | 'after'): void; axis?: 'x' | 'y' }) {
  const [over, setOver] = useState<null | 'into' | 'before' | 'after'>(null);
  const which = (e: React.DragEvent) => {
    const ui = useTodoUi.getState();
    if (ui.drag && onDrop) return 'into' as const;
    if (ui.dragItem && item && ui.dragItem.kind === item.kind) {
      const r = e.currentTarget.getBoundingClientRect();
      return item.axis === 'x' ? halfOf({ top: r.left, height: r.width }, e.clientX) : halfOf(r, e.clientY);
    }
    return null;
  };
  return {
    over,
    props: {
      onDragOver: (e: React.DragEvent) => {
        const w = which(e);
        if (!w) return;
        e.preventDefault();
        e.stopPropagation();
        if (w !== over) setOver(w);
      },
      onDragLeave: (e: React.DragEvent) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(null);
      },
      onDrop: (e: React.DragEvent) => {
        const w = which(e);
        setOver(null);
        if (!w) return;
        e.preventDefault();
        e.stopPropagation();
        const { drag, dragItem } = useTodoUi.getState();
        endDrag();
        if (w === 'into' && drag?.length) onDrop?.(drag);
        else if (dragItem && w !== 'into') item?.onItem(dragItem.id, w);
      },
    },
  };
}

function GroupHead({
  children,
  className,
  drop,
  actions,
  collapsed,
  onToggle,
  dragItem,
}: {
  children: ReactNode;
  className?: string;
  drop?: ReturnType<typeof useDropZone>;
  actions?: ReactNode;
  collapsed?: boolean;
  onToggle?(): void;
  /** Makes the header draggable (reordering sections). */
  dragItem?: DragItem;
}) {
  return (
    <div
      className={clsx('td-group-head', className, drop?.over && `drop-${drop.over}`, dragItem && 'draggable')}
      {...drop?.props}
      draggable={!!dragItem}
      onDragStart={dragItem ? (e) => beginDrag(e, { item: dragItem }) : undefined}
      onDragEnd={dragItem ? endDrag : undefined}
    >
      {dragItem && (
        <span className="td-grip" aria-hidden>
          <GripVertical />
        </span>
      )}
      {onToggle && (
        <button className={clsx('td-caret', collapsed && 'closed')} onClick={onToggle} aria-label={collapsed ? 'Expand' : 'Collapse'}>
          <ChevronDown />
        </button>
      )}
      {children}
      {actions && <span className="td-group-actions">{actions}</span>}
    </div>
  );
}

/** Reorders the list's sections: `moving` before/after `target`. */
export function reorderSections(listId: string, moving: string, target: string, pos: 'before' | 'after') {
  const snap = useTodos.getState().snap;
  const ids = (snap?.sections ?? [])
    .filter((s) => s.listId === listId)
    .sort((a, b) => a.order - b.order)
    .map((s) => s.id);
  const next = reorderIds(ids, moving, target, pos);
  if (next.join() !== ids.join()) void useTodos.getState().apply({ type: 'reorderSections', ids: next });
}

/** Reorders lists (board columns grouped by list, the left pane): `moving` before/after `target`. */
export function reorderLists(moving: string, target: string, pos: 'before' | 'after') {
  const snap = useTodos.getState().snap;
  if (!snap) return;
  const ids = navOrder(snap.lists, snap.projectList).map((l) => l.id);
  const next = reorderIds(ids, moving, target, pos);
  if (next.join() !== ids.join()) void useTodos.getState().apply({ type: 'reorderLists', ids: next });
}

/** Section actions (rename, delete) shared by list sections and board columns. */
export function sectionMenu(section: Section, count: number) {
  const apply = useTodos.getState().apply;
  const rename = async () => {
    const name = await promptDialog({ title: 'Rename section', initial: section.name, confirmText: 'Rename' });
    if (name?.trim()) void apply({ type: 'saveSection', id: section.id, listId: section.listId, name });
  };
  return {
    rename,
    items: [
      { label: 'Rename section…', icon: <Pencil />, onSelect: rename },
      {
        label: 'Delete section…',
        icon: <Trash2 />,
        danger: true,
        onSelect: async () => {
          if (await confirmDialog({ title: `Delete “${section.name}”?`, message: count ? `Its ${count} task${count === 1 ? '' : 's'} will be deleted too.` : undefined, confirmText: 'Delete', danger: true }))
            void apply({ type: 'deleteSection', id: section.id }, `Deleted section ${section.name}`);
        },
      },
    ],
  };
}

/** Moves tasks to the top or the end of a list's slot (no section = `sectionId` null). */
const moveToSlot = (listId: string, sectionId: string | null, label: string, at: 'top' | 'end' = 'end') => (ids: string[]) =>
  void moveTasks(ids, { listId, sectionId, parentId: null, index: at === 'top' ? 0 : null }, `Moved to ${label}`);

function SectionBlock({ section, rows, open, listId, drop }: { section: Section; rows: Row[]; open: number; listId: string; drop?: RowDrop }) {
  const apply = useTodos((s) => s.apply);
  const zone = useDropZone(moveToSlot(listId, section.id, section.name, 'top'), { kind: 'section', onItem: (moving, pos) => reorderSections(listId, moving, section.id, pos) });
  const menu = sectionMenu(section, rows.length);
  return (
    <div className="td-group">
      <GroupHead
        drop={zone}
        dragItem={{ kind: 'section', id: section.id }}
        collapsed={section.collapsed}
        onToggle={() => void apply({ type: 'saveSection', id: section.id, listId, name: '', collapsed: !section.collapsed })}
        actions={
          <>
            <button className="icon-btn sm" onClick={() => useTodoUi.getState().set({ composer: `list:${listId}:${section.id}` })} title="Add task" aria-label="Add task to section">
              <Plus />
            </button>
            <Menu
              align="right"
              items={menu.items}
              trigger={(p) => (
                <button className="icon-btn sm" {...p} aria-label="Section actions">
                  <MoreHorizontal />
                </button>
              )}
            />
          </>
        }
      >
        <span className="truncate" onDoubleClick={menu.rename}>
          {section.name}
        </span>
        <span className="count">{open || ''}</span>
      </GroupHead>
      {!section.collapsed && (
        <>
          <Rows rows={rows} drop={drop} />
          <AddTask slot={`list:${listId}:${section.id}`} defaults={{ listId, sectionId: section.id }} onDropTasks={moveToSlot(listId, section.id, section.name)} />
        </>
      )}
    </div>
  );
}

/** "+ Add section" under a list (or as the last board column). */
export function AddSection({ listId, className }: { listId: string; className?: string }) {
  const apply = useTodos((s) => s.apply);
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  if (!adding)
    return (
      <button className={clsx('td-add-btn', className)} onClick={() => setAdding(true)}>
        <Rows3 /> Add section
      </button>
    );
  const save = () => {
    if (name.trim()) void apply({ type: 'saveSection', listId, name });
    setName('');
    setAdding(false);
  };
  return (
    <div className={clsx('row-sm td-add-section', className)}>
      <input
        className="input sm grow"
        autoFocus
        placeholder="Section name"
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

export function ListView({ listId, view }: { listId: string; view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const showDone = useTodoUi((s) => s.showDone);
  const { sort } = useOptions(view);
  const match = useFilterMatch(view);
  const layout = useMemo(() => listLayout(snap, listId, showDone, sort, match), [snap, listId, showDone, sort, match]);
  // A filter opens collapsed sections that have matches.
  useOrder([...layout.rows, ...layout.sections.flatMap((s) => (s.section.collapsed && !match ? [] : s.rows))].map((r) => r.task.id));
  const inList = useMemo(() => snap.tasks.filter((t) => t.listId === listId), [snap, listId]);
  useViewStats(listStats(inList, showDone, match, olderDoneIn(snap, view)));
  // Sorted by an attribute: positions don't apply, so rows aren't drop targets (headers still are).
  const drop: RowDrop | undefined = sort === 'manual' ? 'tree' : undefined;
  const empty = !layout.rows.length && !layout.sections.length;
  const name = snap.lists.find((l) => l.id === listId)?.name ?? 'list';
  return (
    <div className="td-col">
      <Rows rows={layout.rows} drop={drop} />
      <AddTask slot={`list:${listId}:`} defaults={{ listId }} onDropTasks={moveToSlot(listId, null, name)} />
      {empty && !showDone && !match && (
        <div className="td-empty">
          <Empty icon={<CheckCircle2 />} title="All clear">
            Add a task above — quick add understands <code className="code-inline">tomorrow 5pm</code>, <code className="code-inline">every mon</code>, <code className="code-inline">@label</code>, <code className="code-inline">p1</code> and{' '}
            <code className="code-inline">{'{fri}'}</code> deadlines. Press <kbd className="kbd">Q</kbd> anywhere on this page.
          </Empty>
        </div>
      )}
      {match && !layout.rows.length && layout.sections.every((s) => !s.rows.length) && <div className="td-hint">No tasks match the filter.</div>}
      {layout.sections.map((s) =>
        match && !s.rows.length ? null : <SectionBlock key={s.section.id} section={match ? { ...s.section, collapsed: false } : s.section} rows={s.rows} open={s.open} listId={listId} drop={drop} />,
      )}
      <div className="td-after-sections">
        <AddSection listId={listId} />
      </div>
    </div>
  );
}

function ListGroup({ list, rows, open, drop }: { list: TaskList; rows: Row[]; open: number; drop?: RowDrop }) {
  const zone = useDropZone(moveToSlot(list.id, null, list.name));
  const icon = list.kind === 'inbox' ? <Inbox /> : list.kind === 'project' ? <FolderKanban /> : <Hash />;
  return (
    <div className="td-group">
      <GroupHead drop={zone}>
        <span className="td-group-icon" style={{ color: colorVar(list.color) }}>
          {icon}
        </span>
        <button className="td-group-link truncate" onClick={() => useTodos.getState().setView(list.kind === 'inbox' ? { kind: 'inbox' } : { kind: 'list', id: list.id })} title={`Open ${list.name}`}>
          {list.name}
        </button>
        <span className="count">{open || ''}</span>
      </GroupHead>
      <Rows rows={rows} context="section" drop={drop} />
    </div>
  );
}

/** Every task of every list, grouped by list (left-pane order). */
export function AllTasksView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const showDone = useTodoUi((s) => s.showDone);
  const { sort } = useOptions(view);
  const match = useFilterMatch(view);
  const groups = useMemo(() => allLayout(snap, showDone, sort, match), [snap, showDone, sort, match]);
  useOrder(groups.flatMap((g) => g.rows.map((r) => r.task.id)));
  // Every task of every list: completed ones (and older ones not loaded) and folded sub-tasks are counted and offered.
  useViewStats(listStats(snap.tasks, showDone, match, olderDoneIn(snap, view)));
  const drop: RowDrop | undefined = sort === 'manual' ? 'tree' : undefined;
  return (
    <div className="td-col">
      {groups.map((g) => (
        <ListGroup key={g.list.id} list={g.list} rows={g.rows} open={g.open} drop={drop} />
      ))}
      <AddTask slot="all" defaults={{ listId: snap.projectList ?? 'inbox' }} />
      {!groups.length && match && <div className="td-hint">No tasks match the filter.</div>}
      {!groups.length && !match && (
        <div className="td-empty">
          <Empty icon={<CheckCircle2 />} title={showDone ? 'No tasks yet' : 'No open tasks'}>
            Everything is done — or add a task with <kbd className="kbd">Q</kbd>.
          </Empty>
        </div>
      )}
    </div>
  );
}

function RescheduleButton({ ids, today }: { ids: string[]; today: string }) {
  return (
    <button className="btn btn-sm btn-ghost" onClick={() => void updateTasks(ids, { dueDate: today }, `Rescheduled ${ids.length} task${ids.length === 1 ? '' : 's'} to today`)} title="Move every overdue task to today (keeps times and repeats)">
      <CalendarClock /> Reschedule to today
    </button>
  );
}

/** Rows of date views: dropping tasks on them reschedules to `date` (tasks already on that day stay as they are). */
const toDay = (date: string, today: string): RowDrop => ({
  onDrop: (ids) => {
    const tasks = useTodos.getState().snap?.tasks ?? [];
    const change = ids.filter((id) => {
      const t = tasks.find((x) => x.id === id);
      return t && (t.dueLocal ?? t.due?.date) !== date;
    });
    if (change.length) void updateTasks(change, { dueDate: date }, `Moved to ${dayLabel(date, today)}`);
  },
});

export function TodayView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const today = snap.today;
  const tasks = useFiltered(view, useMemo(() => attention(snap.tasks, today), [snap, today]));
  const g = useMemo(() => todayGroups(tasks, today), [tasks, today]);
  const [showOverdue, setShowOverdue] = useState(true);
  useOrder([...(showOverdue ? g.overdue : []), ...g.today].map((t) => t.id));
  const dropToday = useDropZone((ids) => void updateTasks(ids, { dueDate: today }, 'Moved to today'));
  return (
    <div className="td-col">
      {g.overdue.length > 0 && (
        <div className="td-group">
          <GroupHead className="overdue" collapsed={!showOverdue} onToggle={() => setShowOverdue(!showOverdue)} actions={<RescheduleButton ids={g.overdue.map((t) => t.id)} today={today} />}>
            Overdue <span className="count">{g.overdue.length}</span>
          </GroupHead>
          {showOverdue && <Rows rows={flat(g.overdue)} context="list" />}
        </div>
      )}
      <div className="td-group">
        <GroupHead drop={dropToday}>
          {dayHeading(today, today)} <span className="count">{g.today.length || ''}</span>
        </GroupHead>
        <Rows rows={flat(g.today)} context="list" dueMode="time" drop={toDay(today, today)} />
        <AddTask slot="today" defaults={{ due: { date: today }, listId: snap.projectList ?? 'inbox' }} />
      </div>
      {!g.overdue.length && !g.today.length && (
        <div className="td-empty">
          <Empty icon={<CheckCircle2 />} title="Nothing due today">
            Enjoy it — or plan ahead in Upcoming.
          </Empty>
        </div>
      )}
    </div>
  );
}

function DayGroup({ date, tasks, today }: { date: string; tasks: TaskView[]; today: string }) {
  const drop = useDropZone((ids) => void updateTasks(ids, { dueDate: date }, `Moved to ${dayLabel(date, today)}`));
  return (
    <div className="td-group" id={`td-day-${date}`}>
      <GroupHead drop={drop} className={clsx(!tasks.length && 'empty-day')}>
        {dayHeading(date, today)} <span className="count">{tasks.length || ''}</span>
      </GroupHead>
      <Rows rows={flat(tasks)} context="list" dueMode="time" drop={toDay(date, today)} />
      <AddTask slot={`day:${date}`} defaults={{ due: { date } }} />
    </div>
  );
}

function WeekStrip({ start, setStart, today, counts }: { start: string; setStart(s: string): void; today: string; counts: Map<string, number> }) {
  return (
    <div className="td-week">
      {weekDays(start).map((d) => (
        <WeekDay key={d} d={d} today={today} count={counts.get(d) ?? 0} />
      ))}
      <div className="td-week-nav">
        <span className="small strong grow truncate">{parseYmd(start).toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
        <button className="icon-btn sm" onClick={() => setStart(addDays(start, -7))} aria-label="Previous week" disabled={start <= startOfWeek(today)}>
          <ChevronLeft />
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => setStart(startOfWeek(today))}>
          Today
        </button>
        <button className="icon-btn sm" onClick={() => setStart(addDays(start, 7))} aria-label="Next week">
          <ChevronRight />
        </button>
      </div>
    </div>
  );
}

function WeekDay({ d, today, count }: { d: string; today: string; count: number }) {
  const drop = useDropZone((ids) => void updateTasks(ids, { dueDate: d }, `Moved to ${dayLabel(d, today)}`));
  return (
    <button className={clsx('td-day', d === today && 'today', d < today && 'past', drop.over && 'drop')} {...drop.props} onClick={() => document.getElementById(`td-day-${d}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })} title={dayHeading(d, today)}>
      <span>{parseYmd(d).toLocaleDateString(undefined, { weekday: 'short' })}</span>
      <span className="n">{Number(d.slice(8))}</span>
      <span className="dots">
        {Array.from({ length: Math.min(4, count) }, (_, i) => (
          <i key={i} />
        ))}
      </span>
    </button>
  );
}

export function UpcomingView({ view }: { view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const today = snap.today;
  const [start, setStart] = useState(() => startOfWeek(today));
  const [days, setDays] = useState(28);
  const end = addDays(start, 6);
  const need = Math.max(days, Math.min(366, 1 + Math.round((parseYmd(end).getTime() - parseYmd(today).getTime()) / 86_400_000)));
  const dated = useFiltered(view, useMemo(() => snap.tasks.filter((t) => !t.done && (t.dueLocal ?? t.due?.date)), [snap]));
  const data = useMemo(() => upcomingDays(dated, today, need), [dated, today, need]);
  const counts = useMemo(() => new Map(data.days.map((d) => [d.date, d.tasks.length])), [data]);
  useOrder([...data.overdue, ...data.days.flatMap((d) => d.tasks)].map((t) => t.id));
  useEffect(() => {
    if (start >= today) document.getElementById(`td-day-${start}`)?.scrollIntoView({ block: 'start' });
  }, [start, today]);
  return (
    <div className="td-col">
      <WeekStrip start={start} setStart={setStart} today={today} counts={counts} />
      {data.overdue.length > 0 && (
        <div className="td-group">
          <GroupHead className="overdue" actions={<RescheduleButton ids={data.overdue.map((t) => t.id)} today={today} />}>
            Overdue <span className="count">{data.overdue.length}</span>
          </GroupHead>
          <Rows rows={flat(data.overdue)} context="list" />
        </div>
      )}
      {data.days.map((d) => (
        <DayGroup key={d.date} date={d.date} tasks={d.tasks} today={today} />
      ))}
      <button className="td-add-btn" onClick={() => setDays(need + 28)}>
        <ChevronDown /> Show 4 more weeks
      </button>
    </div>
  );
}

export function LabelView({ name, view }: { name: string; view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const tasks = useFiltered(view, useMemo(() => snap.tasks.filter((t) => !t.done && t.labels?.includes(name)).sort(byDue), [snap, name]));
  useOrder(tasks.map((t) => t.id));
  return (
    <div className="td-col">
      <Rows rows={flat(tasks)} context="list" />
      <AddTask slot={`label:${name}`} defaults={{ labels: [name], listId: snap.projectList ?? 'inbox' }} />
      {!tasks.length && <div className="td-hint">No open tasks with @{name}.</div>}
    </div>
  );
}

export function FilterResults({ id, view }: { id: string; view: ViewRef }) {
  const snap = useTodos((s) => s.snap)!;
  const f = snap.filters.find((x) => x.id === id);
  const byId = useMemo(() => new Map(snap.tasks.map((t) => [t.id, t])), [snap]);
  const all = useMemo(() => (f?.groups ?? []).map((g) => ({ title: g.title, tasks: g.ids.map((i) => byId.get(i)).filter((t): t is TaskView => !!t) })), [f, byId]);
  const match = useFilterMatch(view);
  const groups = useMemo(() => (match ? all.map((g) => ({ ...g, tasks: g.tasks.filter(match) })) : all), [all, match]);
  const count = (gs: typeof all) => new Set(gs.flatMap((g) => g.tasks.map((t) => t.id))).size;
  useViewStats({ shown: count(groups), total: count(all), unit: 'tasks', hidden: [] });
  useOrder(groups.flatMap((g) => g.tasks.map((t) => t.id)));
  if (!f) return null;
  if (f.error) return <div className="callout danger td-callout">{f.error}</div>;
  return (
    <div className="td-col">
      {groups.map((g, i) => (
        <div key={i} className="td-group">
          {groups.length > 1 && (
            <GroupHead>
              <span className="mono truncate">{g.title}</span> <span className="count">{g.tasks.length}</span>
            </GroupHead>
          )}
          <Rows rows={flat(g.tasks)} context="list" />
          {!g.tasks.length && <div className="td-hint">Nothing matches.</div>}
        </div>
      ))}
    </div>
  );
}

export function SearchResults({ q }: { q: string }) {
  const snap = useTodos((s) => s.snap)!;
  const allDone = useTodos((s) => s.allDone);
  const tasks = useMemo(() => searchTasks(snap.tasks, q).sort((a, b) => Number(!!a.done) - Number(!!b.done) || byDue(a, b)), [snap, q]);
  useOrder(tasks.map((t) => t.id));
  // Search covers the snapshot: older completed tasks only once loaded.
  const older = allDone ? 0 : Object.values(snap.olderDone ?? {}).reduce((a, n) => a + n, 0);
  useViewStats({ shown: tasks.length, total: tasks.length, unit: 'tasks', hidden: [{ kind: 'older', count: older }] });
  return (
    <div className="td-col">
      <Rows rows={flat(tasks)} context="list" />
      {!tasks.length && <div className="td-hint">No tasks match “{q}”.</div>}
    </div>
  );
}

export function CompletedView() {
  const snap = useTodos((s) => s.snap)!;
  const [entries, setEntries] = useState<CompletedEntry[] | null>(null);
  const [more, setMore] = useState(true);
  const load = useCallback(async (before?: number) => {
    const page = await todosApi.completed(null, before, 100).catch(() => []);
    setEntries((prev) => (before ? [...(prev ?? []), ...page] : page));
    setMore(page.length === 100);
  }, []);
  useEffect(() => {
    void load();
  }, [load, snap.completedCount]);
  useOrder([]);
  useViewStats(null);
  if (!entries) return <Spinner />;
  const days = new Map<string, CompletedEntry[]>();
  for (const e of entries) {
    const d = ymd(new Date(e.at));
    days.set(d, [...(days.get(d) ?? []), e]);
  }
  // Only the latest completion of a recurring task can be undone.
  const latest = new Map<string, number>();
  for (const e of entries) if (e.recurring && !latest.has(e.task.id)) latest.set(e.task.id, e.at);
  return (
    <div className="td-col">
      {!entries.length && (
        <div className="td-empty">
          <Empty icon={<CheckCircle2 />} title="Nothing completed yet" />
        </div>
      )}
      {[...days].map(([d, list]) => (
        <div key={d} className="td-group">
          <GroupHead>
            {dayHeading(d, snap.today)} <span className="count">{list.length}</span>
          </GroupHead>
          <div className="td-rows">
            {list.map((e) => {
              const l = snap.lists.find((x) => x.id === e.task.listId);
              const canUndo = e.recurring ? latest.get(e.task.id) === e.at && !e.task.done : !!e.task.done;
              return (
                <div key={`${e.task.id}${e.at}`} className="td-row done" onClick={() => useTodos.getState().open(e.task.id)}>
                  <span className={clsx('td-check on', `p${e.task.priority}`)} aria-hidden>
                    {e.recurring ? <Repeat /> : <CheckCircle2 />}
                  </span>
                  <div className="td-main-col">
                    <span className="td-content">
                      <InlineText text={e.task.content} />
                    </span>
                    <div className="td-meta">
                      <span>{new Date(e.at).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}</span>
                      {e.recurring && e.occurrence && <span className="truncate">occurrence of {dayLabel(e.occurrence, snap.today)}</span>}
                      <span className="td-listname">
                        <span className="truncate">{l?.name}</span>
                      </span>
                    </div>
                  </div>
                  {canUndo && (
                    <button
                      className="btn btn-sm btn-ghost"
                      onClick={(ev) => {
                        ev.stopPropagation();
                        void completeTasks([e.task.id], false);
                      }}
                      title={e.recurring ? 'Undo this completion (moves the date back)' : 'Reopen'}
                    >
                      <Undo2 /> {e.recurring ? 'Undo' : 'Reopen'}
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      ))}
      {more && entries.length > 0 && (
        <button className="td-add-btn" onClick={() => void load(entries[entries.length - 1]?.at)}>
          <ChevronDown /> Older
        </button>
      )}
    </div>
  );
}
