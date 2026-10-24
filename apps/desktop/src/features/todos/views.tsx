import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { CalendarClock, CheckCircle2, ChevronDown, ChevronLeft, ChevronRight, MoreHorizontal, Pencil, Plus, Repeat, Rows3, Trash2, Undo2 } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import { Empty, Spinner } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { todosApi, type CompletedEntry, type Section, type TaskView } from '@/lib/api/todos';
import { completeTasks, moveTasks, updateTasks, useTodos } from '@/store/todos';
import { AddTask, InlineText, TaskRow } from './TaskRow';
import { addDays, byDue, dayHeading, dayLabel, listLayout, parseYmd, searchTasks, startOfWeek, todayGroups, upcomingDays, weekDays, ymd, type Row } from './lib';
import { useTodoUi } from './ui';

/** Publishes the view's row order (for j/k and Shift-click). */
function useOrder(ids: string[]) {
  const key = ids.join(',');
  useEffect(() => {
    useTodoUi.getState().set({ order: key ? key.split(',') : [] });
  }, [key]);
}

const flat = (tasks: TaskView[]): Row[] => tasks.map((task) => ({ task, depth: 0, open: 0, kids: 0 }));

function Rows({ rows, showList, dueMode }: { rows: Row[]; showList?: boolean; dueMode?: 'full' | 'time' }) {
  return (
    <div className="td-rows">
      {rows.map((r) => (
        <TaskRow key={r.task.id} row={r} showList={showList} dueMode={dueMode} />
      ))}
    </div>
  );
}

/** Dropping dragged tasks on a header: `onDrop(ids)`. */
function useHeaderDrop(onDrop: (ids: string[]) => void) {
  const [over, setOver] = useState(false);
  return {
    over,
    props: {
      onDragOver: (e: React.DragEvent) => {
        if (!useTodoUi.getState().drag) return;
        e.preventDefault();
        setOver(true);
      },
      onDragLeave: () => setOver(false),
      onDrop: (e: React.DragEvent) => {
        e.preventDefault();
        setOver(false);
        const ids = useTodoUi.getState().drag;
        useTodoUi.getState().set({ drag: null, drop: null });
        if (ids?.length) onDrop(ids);
      },
    },
  };
}

function GroupHead({ children, className, drop, actions, collapsed, onToggle }: { children: ReactNode; className?: string; drop?: ReturnType<typeof useHeaderDrop>; actions?: ReactNode; collapsed?: boolean; onToggle?(): void }) {
  return (
    <div className={clsx('td-group-head', className, drop?.over && 'drop')} {...drop?.props}>
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

function SectionBlock({ section, rows, open, listId }: { section: Section; rows: Row[]; open: number; listId: string }) {
  const apply = useTodos((s) => s.apply);
  const drop = useHeaderDrop((ids) => void moveTasks(ids, { listId, sectionId: section.id, parentId: null, index: 0 }, `Moved to ${section.name}`));
  const rename = async () => {
    const name = await promptDialog({ title: 'Rename section', initial: section.name, confirmText: 'Rename' });
    if (name?.trim()) void apply({ type: 'saveSection', id: section.id, listId, name });
  };
  return (
    <div className="td-group">
      <GroupHead
        drop={drop}
        collapsed={section.collapsed}
        onToggle={() => void apply({ type: 'saveSection', id: section.id, listId, name: '', collapsed: !section.collapsed })}
        actions={
          <>
            <button className="icon-btn sm" onClick={() => useTodoUi.getState().set({ composer: `list:${listId}:${section.id}` })} title="Add task" aria-label="Add task to section">
              <Plus />
            </button>
            <Menu
              align="right"
              items={[
                { label: 'Rename section…', icon: <Pencil />, onSelect: rename },
                {
                  label: 'Delete section…',
                  icon: <Trash2 />,
                  danger: true,
                  onSelect: async () => {
                    const n = rows.length;
                    if (await confirmDialog({ title: `Delete “${section.name}”?`, message: n ? `Its ${n} task${n === 1 ? '' : 's'} will be deleted too.` : undefined, confirmText: 'Delete', danger: true }))
                      void apply({ type: 'deleteSection', id: section.id }, `Deleted section ${section.name}`);
                  },
                },
              ]}
              trigger={(p) => (
                <button className="icon-btn sm" {...p} aria-label="Section actions">
                  <MoreHorizontal />
                </button>
              )}
            />
          </>
        }
      >
        <span onDoubleClick={rename}>{section.name}</span>
        <span className="count">{open || ''}</span>
      </GroupHead>
      {!section.collapsed && (
        <>
          <Rows rows={rows} />
          <AddTask slot={`list:${listId}:${section.id}`} defaults={{ listId, sectionId: section.id }} />
        </>
      )}
    </div>
  );
}

export function ListView({ listId }: { listId: string }) {
  const snap = useTodos((s) => s.snap)!;
  const apply = useTodos((s) => s.apply);
  const showDone = useTodoUi((s) => s.showDone);
  const layout = useMemo(() => listLayout(snap, listId, showDone), [snap, listId, showDone]);
  useOrder([...layout.rows, ...layout.sections.flatMap((s) => (s.section.collapsed ? [] : s.rows))].map((r) => r.task.id));
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const empty = !layout.rows.length && !layout.sections.length;
  return (
    <div className="td-col">
      <Rows rows={layout.rows} />
      <AddTask slot={`list:${listId}:`} defaults={{ listId }} />
      {empty && !showDone && (
        <div className="td-empty">
          <Empty icon={<CheckCircle2 />} title="All clear">
            Add a task above — quick add understands <code className="code-inline">tomorrow 5pm</code>, <code className="code-inline">every mon</code>, <code className="code-inline">@label</code>, <code className="code-inline">p1</code> and{' '}
            <code className="code-inline">{'{fri}'}</code> deadlines. Press <kbd className="kbd">Q</kbd> anywhere on this page.
          </Empty>
        </div>
      )}
      {layout.sections.map((s) => (
        <SectionBlock key={s.section.id} section={s.section} rows={s.rows} open={s.open} listId={listId} />
      ))}
      <div style={{ marginTop: 'var(--space-6)' }}>
        {adding ? (
          <div className="row-sm">
            <input
              className="input sm"
              autoFocus
              placeholder="Section name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === 'Enter' && name.trim()) {
                  void apply({ type: 'saveSection', listId, name });
                  setName('');
                  setAdding(false);
                }
                if (e.key === 'Escape') setAdding(false);
              }}
              style={{ maxWidth: 320 }}
            />
            <button className="btn btn-sm btn-ghost" onClick={() => setAdding(false)}>
              Cancel
            </button>
          </div>
        ) : (
          <button className="td-add-btn" onClick={() => setAdding(true)}>
            <Rows3 /> Add section
          </button>
        )}
      </div>
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

export function TodayView() {
  const snap = useTodos((s) => s.snap)!;
  const today = snap.today;
  const g = useMemo(() => todayGroups(snap.tasks, today), [snap, today]);
  const [showOverdue, setShowOverdue] = useState(true);
  useOrder([...(showOverdue ? g.overdue : []), ...g.today].map((t) => t.id));
  const dropToday = useHeaderDrop((ids) => void updateTasks(ids, { dueDate: today }, 'Moved to today'));
  return (
    <div className="td-col">
      {g.overdue.length > 0 && (
        <div className="td-group">
          <GroupHead className="overdue" collapsed={!showOverdue} onToggle={() => setShowOverdue(!showOverdue)} actions={<RescheduleButton ids={g.overdue.map((t) => t.id)} today={today} />}>
            Overdue <span className="count">{g.overdue.length}</span>
          </GroupHead>
          {showOverdue && <Rows rows={flat(g.overdue)} showList />}
        </div>
      )}
      <div className="td-group">
        <GroupHead drop={dropToday}>
          {dayHeading(today, today)} <span className="count">{g.today.length || ''}</span>
        </GroupHead>
        <Rows rows={flat(g.today)} showList dueMode="time" />
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
  const drop = useHeaderDrop((ids) => void updateTasks(ids, { dueDate: date }, `Moved to ${dayLabel(date, today)}`));
  return (
    <div className="td-group" id={`td-day-${date}`}>
      <GroupHead drop={drop} className={clsx(!tasks.length && 'empty-day')}>
        {dayHeading(date, today)} <span className="count">{tasks.length || ''}</span>
      </GroupHead>
      <Rows rows={flat(tasks)} showList dueMode="time" />
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
      <div style={{ gridColumn: '1 / -1', display: 'flex', alignItems: 'center', gap: 'var(--space-2)' }}>
        <span className="small strong grow">{parseYmd(start).toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
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
  const drop = useHeaderDrop((ids) => void updateTasks(ids, { dueDate: d }, `Moved to ${dayLabel(d, today)}`));
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

export function UpcomingView() {
  const snap = useTodos((s) => s.snap)!;
  const today = snap.today;
  const [start, setStart] = useState(() => startOfWeek(today));
  const [days, setDays] = useState(28);
  const end = addDays(start, 6);
  const need = Math.max(days, Math.min(366, 1 + Math.round((parseYmd(end).getTime() - parseYmd(today).getTime()) / 86_400_000)));
  const data = useMemo(() => upcomingDays(snap.tasks, today, need), [snap, today, need]);
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
          <Rows rows={flat(data.overdue)} showList />
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

export function LabelView({ name }: { name: string }) {
  const snap = useTodos((s) => s.snap)!;
  const tasks = useMemo(() => snap.tasks.filter((t) => !t.done && t.labels?.includes(name)).sort(byDue), [snap, name]);
  useOrder(tasks.map((t) => t.id));
  return (
    <div className="td-col">
      <Rows rows={flat(tasks)} showList />
      <AddTask slot={`label:${name}`} defaults={{ labels: [name], listId: snap.projectList ?? 'inbox' }} />
      {!tasks.length && <div className="td-hint">No open tasks with @{name}.</div>}
    </div>
  );
}

export function FilterResults({ id }: { id: string }) {
  const snap = useTodos((s) => s.snap)!;
  const f = snap.filters.find((x) => x.id === id);
  const byId = useMemo(() => new Map(snap.tasks.map((t) => [t.id, t])), [snap]);
  const groups = useMemo(() => (f?.groups ?? []).map((g) => ({ title: g.title, tasks: g.ids.map((i) => byId.get(i)).filter((t): t is TaskView => !!t) })), [f, byId]);
  useOrder(groups.flatMap((g) => g.tasks.map((t) => t.id)));
  if (!f) return null;
  if (f.error) return <div className="callout danger">{f.error}</div>;
  return (
    <div className="td-col">
      {groups.map((g, i) => (
        <div key={i} className="td-group">
          {groups.length > 1 && (
            <GroupHead>
              <span className="mono">{g.title}</span> <span className="count">{g.tasks.length}</span>
            </GroupHead>
          )}
          <Rows rows={flat(g.tasks)} showList />
          {!g.tasks.length && <div className="td-hint">Nothing matches.</div>}
        </div>
      ))}
    </div>
  );
}

export function SearchResults({ q }: { q: string }) {
  const snap = useTodos((s) => s.snap)!;
  const tasks = useMemo(() => searchTasks(snap.tasks, q).sort((a, b) => Number(!!a.done) - Number(!!b.done) || byDue(a, b)), [snap, q]);
  useOrder(tasks.map((t) => t.id));
  return (
    <div className="td-col">
      <Rows rows={flat(tasks)} showList />
      {!tasks.length && <div className="td-hint">No tasks match “{q}”. Completed tasks older than a week are in Completed.</div>}
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
                      {e.recurring && e.occurrence && <span>occurrence of {dayLabel(e.occurrence, snap.today)}</span>}
                      <span className="td-listname">{l?.name}</span>
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

