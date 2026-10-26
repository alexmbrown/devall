/**
 * Pure helpers for the Board layout: which columns a view gets, the cards in each, and what
 * dropping cards does (a `status` change — the default grouping, workflow columns such as To do /
 * In progress / Done — a `move` into a list/section, or an `update` of priority or due date).
 * Also the status (column) helpers shared with list rows and the detail panel. No React — tested
 * in `boardModel.test.ts`.
 */
import type { NewTask, Priority, Section, Snapshot, StatusColumn, Task, TaskList, TaskPatch, TaskView } from '@/lib/api/todos';
import { addDays, attention, byDue, compareBy, dayHeading, dropTarget, dueDay, markdownChecklist, navOrder, ymd, type MoveTarget, type SortKey, type ViewRef } from './lib';

export type Layout = 'list' | 'board';
export type GroupBy = 'status' | 'section' | 'list' | 'priority' | 'due';

export const GROUP_LABELS: Record<GroupBy, string> = { status: 'Status', section: 'Section', list: 'List', priority: 'Priority', due: 'Due date' };

/** Board groupings that make sense for a view; the first (status columns) is the default. */
export function groupChoices(view: ViewRef): GroupBy[] {
  switch (view.kind) {
    case 'list':
    case 'inbox':
      return ['status', 'section', 'priority', 'due'];
    case 'all':
      return ['status', 'list', 'priority', 'due'];
    case 'upcoming':
      return ['status', 'due', 'priority', 'list'];
    default:
      return ['status', 'priority', 'due', 'list'];
  }
}

// ── statuses (board columns) ─────────────────────────────────────────────────────────────────────

/** Where new and unstatused tasks land: the first column that isn't the done column. */
export const firstColumn = (cols: StatusColumn[]) => cols.find((c) => !c.done);
export const doneColumn = (cols: StatusColumn[]) => cols.find((c) => c.done);

/** The column a task is in (mirrors `TodoSettings::status_of`): done → the done column; unknown or missing → the first column. */
export function statusOf(t: Pick<Task, 'done' | 'status'>, cols: StatusColumn[]): StatusColumn | undefined {
  if (t.done) return doneColumn(cols);
  return cols.find((c) => c.id === t.status && !c.done) ?? firstColumn(cols);
}

/** The status chip list rows show: only for columns between the first and the done one ("In progress"). */
export function statusChip(t: Pick<Task, 'done' | 'status'>, cols: StatusColumn[]): StatusColumn | undefined {
  const c = statusOf(t, cols);
  return c && !c.done && c.id !== firstColumn(cols)?.id ? c : undefined;
}

/**
 * Manual order inside a status column (mirrors `TodoData::column_tasks`): the position set by
 * dragging, then list (the snapshot's order: Inbox first), section and position in the list.
 */
export function columnOrder(snap: Pick<Snapshot, 'lists' | 'sections'>): (a: Task, b: Task) => number {
  const listRank = new Map(snap.lists.map((l, i) => [l.id, i]));
  const sectionRank = new Map(snap.sections.map((s) => [s.id, s.order + 1]));
  const key = (t: Task) => [t.statusOrder ?? 0, listRank.get(t.listId) ?? Number.MAX_SAFE_INTEGER, (t.sectionId && sectionRank.get(t.sectionId)) || 0, t.order];
  return (a, b) => {
    const ka = key(a);
    const kb = key(b);
    for (let i = 0; i < ka.length; i++) if (ka[i] !== kb[i]) return ka[i] - kb[i];
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  };
}

/** Open tasks per column id (what removing a column would move). */
export function columnCounts(tasks: Pick<Task, 'done' | 'status'>[], cols: StatusColumn[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const t of tasks) {
    if (t.done) continue;
    const c = statusOf(t, cols);
    if (c) out.set(c.id, (out.get(c.id) ?? 0) + 1);
  }
  return out;
}

/** `cols` with `id` moved one place left (-1) or right (+1). */
export function moveColumn(cols: StatusColumn[], id: string, delta: -1 | 1): StatusColumn[] {
  const i = cols.findIndex((c) => c.id === id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= cols.length) return cols;
  const out = [...cols];
  [out[i], out[j]] = [out[j], out[i]];
  return out;
}

/** `cols` with a new column `name`, before the done column when that one is last. */
export function insertColumn(cols: StatusColumn[], name: string): StatusColumn[] {
  const col: StatusColumn = { id: '', name: name.trim() };
  const at = cols.length && cols[cols.length - 1].done ? cols.length - 1 : cols.length;
  return [...cols.slice(0, at), col, ...cols.slice(at)];
}

/** Problems with an edited column list (what Rust refuses too), or null. */
export function columnsError(cols: StatusColumn[]): string | null {
  if (cols.some((c) => !c.name.trim())) return 'Every column needs a name.';
  const names = cols.map((c) => c.name.trim().replace(/\s+/g, ' ').toLowerCase());
  const dup = names.find((n, i) => names.indexOf(n) !== i);
  if (dup) return `Two columns are called “${cols[names.indexOf(dup)].name.trim()}”.`;
  if (cols.filter((c) => c.done).length !== 1) return 'Pick exactly one done column.';
  if (cols.length < 2) return 'Add a column besides the done column.';
  if (cols.length > 20) return 'A board has at most 20 columns.';
  return null;
}

/** Views that can be shown as a board (Completed is a log). */
export const canBoard = (view: ViewRef) => view.kind !== 'completed';

/** Where cards dropped into a column go. */
export type ColumnTarget =
  /** A status column (`done`: dropping completes). */
  | { kind: 'status'; status: string; done: boolean }
  /** A list (`sectionId` null = no section). `wholeList`: the column shows every section of the list. */
  | { kind: 'slot'; listId: string; sectionId: string | null; wholeList?: boolean }
  | { kind: 'priority'; priority: Priority }
  /** `''` = no date. */
  | { kind: 'due'; date: string }
  /** Overdue / Later: nothing sensible to set. */
  | { kind: 'none' };

export interface BoardColumn {
  /** `status:<id>`, `section:<id>`, `section:` (no section), `list:<id>`, `p1`…`p4`, `due:<YYYY-MM-DD>`, `due:` (no date), `due:overdue`, `due:later`. */
  key: string;
  title: string;
  tasks: TaskView[];
  target: ColumnTarget;
  /** Cards keep their manual order: they can be dragged within the column. */
  ordered: boolean;
  /** For headers: the status, section or list the column stands for (rename, reorder). */
  column?: StatusColumn;
  section?: Section;
  list?: TaskList;
  priority?: Priority;
  tone?: 'overdue';
}

/** Sub-tasks on a board: their own cards, or inside the parent's card when in the parent's column. */
export type SubtaskMode = 'cards' | 'nested';

export interface BoardCtx {
  /** The list a list/Inbox view shows (section columns). */
  listId?: string;
  sort: SortKey;
  today: string;
  /** Default `nested`. */
  subtasks?: SubtaskMode;
}

/** The tasks a view shows (before grouping): completed ones only in list-like views with `showDone`. */
export function viewTasks(snap: Snapshot, view: ViewRef, showDone: boolean): TaskView[] {
  const shown = (t: TaskView) => showDone || !t.done;
  switch (view.kind) {
    case 'inbox':
      return snap.tasks.filter((t) => t.listId === 'inbox' && shown(t));
    case 'list':
      return snap.tasks.filter((t) => t.listId === view.id && shown(t));
    case 'all':
      return snap.tasks.filter(shown);
    case 'today':
      return attention(snap.tasks, snap.today);
    case 'upcoming':
      return snap.tasks.filter((t) => !t.done && !!dueDay(t));
    case 'label':
      return snap.tasks.filter((t) => !t.done && !!t.labels?.includes(view.name));
    case 'filter': {
      const ids = new Set(snap.filters.find((f) => f.id === view.id)?.groups.flatMap((g) => g.ids) ?? []);
      return snap.tasks.filter((t) => ids.has(t.id));
    }
    default:
      return [];
  }
}

/** Completed tasks a view's done column can show (filters only ever match open tasks). */
function doneInView(snap: Snapshot, view: ViewRef, t: TaskView): boolean {
  switch (view.kind) {
    case 'inbox':
      return t.listId === 'inbox';
    case 'list':
      return t.listId === view.id;
    case 'all':
      return true;
    case 'today':
      return (dueDay(t) ?? '9999') <= snap.today;
    case 'upcoming':
      return !!dueDay(t);
    case 'label':
      return !!t.labels?.includes(view.name);
    default:
      return false;
  }
}

/** Completed tasks of the view that a status board's done column can show (any age). */
export function doneTasksInView(snap: Snapshot, view: ViewRef): TaskView[] {
  return snap.tasks.filter((t) => t.done && doneInView(snap, view, t));
}

/** Whether a completed task is inside the done column's window (`doneDays` 0 = every completed task). */
export function inDoneWindow(t: Pick<Task, 'completedAt'>, today: string, doneDays: number): boolean {
  if (doneDays <= 0) return true;
  return !!t.completedAt && ymd(new Date(t.completedAt)) >= addDays(today, -(Math.max(1, doneDays) - 1));
}

/**
 * The tasks a board shows (before grouping). Status boards show the view's open tasks plus those
 * completed in the last `doneDays` days (0: all; the done column, whatever "show completed" says);
 * other groupings show completed tasks only with `showDone`.
 */
export function boardTasks(snap: Snapshot, view: ViewRef, groupBy: GroupBy, showDone: boolean, doneDays: number): TaskView[] {
  if (groupBy !== 'status') return viewTasks(snap, view, showDone);
  return [...viewTasks(snap, view, false), ...doneTasksInView(snap, view).filter((t) => inDoneWindow(t, snap.today, doneDays))];
}

/** Drops tasks whose parent is shown too: they appear as a count on the parent's card. */
export function topLevel(tasks: TaskView[]): TaskView[] {
  const ids = new Set(tasks.map((t) => t.id));
  return tasks.filter((t) => !t.parentId || !ids.has(t.parentId));
}

/**
 * The column key a task's card belongs to under `groupBy` (see `BoardColumn.key`), or null when the
 * board has no column for it (another list's task on a section board).
 */
export function columnKeyOf(snap: Pick<Snapshot, 'sections' | 'settings'>, groupBy: GroupBy, ctx: Pick<BoardCtx, 'listId' | 'today'>): (t: TaskView) => string | null {
  switch (groupBy) {
    case 'status':
      return (t) => {
        const c = statusOf(t, snap.settings.columns);
        return c ? `status:${c.id}` : null;
      };
    case 'section': {
      const listId = ctx.listId ?? 'inbox';
      const known = new Set(snap.sections.filter((s) => s.listId === listId).map((s) => s.id));
      return (t) => (t.listId !== listId ? null : `section:${t.sectionId && known.has(t.sectionId) ? t.sectionId : ''}`);
    }
    case 'list':
      return (t) => `list:${t.listId}`;
    case 'priority':
      return (t) => `p${t.priority}`;
    case 'due': {
      const last = addDays(ctx.today, 6);
      return (t) => {
        const d = dueDay(t);
        return !d ? 'due:' : d < ctx.today ? 'due:overdue' : d > last ? 'due:later' : `due:${d}`;
      };
    }
  }
}

/**
 * Which tasks get a card. `cards`: every task. `nested`: a sub-task whose parent has a card in the
 * same column shows inside it (the checklist count) — `folded` — instead of on its own; sub-tasks in
 * another column than their parent always get a card (they'd be invisible otherwise).
 */
export function foldSubtasks(tasks: TaskView[], keyOf: (t: TaskView) => string | null, mode: SubtaskMode): { cards: TaskView[]; folded: TaskView[] } {
  if (mode === 'cards') return { cards: tasks, folded: [] };
  const byId = new Map(tasks.map((t) => [t.id, t]));
  const cards: TaskView[] = [];
  const folded: TaskView[] = [];
  for (const t of tasks) {
    const parent = t.parentId ? byId.get(t.parentId) : undefined;
    if (parent && keyOf(parent) === keyOf(t)) folded.push(t);
    else cards.push(t);
  }
  return { cards, folded };
}

/** A card's checklist: its sub-tasks plus the task list in its description (`- [ ]`), as "done/total". */
export function cardChecklist(t: Pick<Task, 'description'>, kids?: { open: number; all: number }): { done: number; total: number } {
  const md = markdownChecklist(t.description);
  return { done: md.done + (kids ? kids.all - kids.open : 0), total: md.total + (kids?.all ?? 0) };
}

/** Open / all sub-tasks per parent id. */
export function childCounts(tasks: Task[]): Map<string, { open: number; all: number }> {
  const out = new Map<string, { open: number; all: number }>();
  for (const t of tasks) {
    if (!t.parentId) continue;
    const c = out.get(t.parentId) ?? { open: 0, all: 0 };
    c.all++;
    if (!t.done) c.open++;
    out.set(t.parentId, c);
  }
  return out;
}

const doneLast = (cmp: (a: TaskView, b: TaskView) => number) => (a: TaskView, b: TaskView) => Number(!!a.done) - Number(!!b.done) || cmp(a, b);

/**
 * The columns for `tasks` (what the view shows, completed ones included only when wanted) grouped
 * by `groupBy`. Section and list columns are drop targets even when empty; "Overdue" and "Later"
 * only appear when they have cards.
 */
export function boardColumns(snap: Snapshot, tasks: TaskView[], groupBy: GroupBy, ctx: BoardCtx): BoardColumn[] {
  const { cards } = foldSubtasks(tasks, columnKeyOf(snap, groupBy, ctx), ctx.subtasks ?? 'nested');
  const manual = ctx.sort === 'manual';
  const cmp = doneLast(compareBy(ctx.sort));
  switch (groupBy) {
    case 'status': {
      // Every column is a drop target, empty or not; the done column lists the latest completions first.
      const cols = snap.settings.columns;
      const order = columnOrder(snap);
      const byStatus = new Map<string, TaskView[]>();
      for (const t of cards) {
        const c = statusOf(t, cols);
        if (c) byStatus.set(c.id, [...(byStatus.get(c.id) ?? []), t]);
      }
      return cols.map((column): BoardColumn => {
        const mine = byStatus.get(column.id) ?? [];
        const sorted = column.done ? mine.sort((a, b) => (b.completedAt ?? 0) - (a.completedAt ?? 0)) : mine.sort(manual ? order : cmp);
        return { key: `status:${column.id}`, title: column.name, tasks: sorted, target: { kind: 'status', status: column.id, done: !!column.done }, ordered: manual && !column.done, column };
      });
    }
    case 'section': {
      const listId = ctx.listId ?? 'inbox';
      const sections = snap.sections.filter((s) => s.listId === listId).sort((a, b) => a.order - b.order);
      const known = new Set(sections.map((s) => s.id));
      const inList = cards.filter((t) => t.listId === listId);
      return [
        { key: 'section:', title: 'No section', tasks: inList.filter((t) => !t.sectionId || !known.has(t.sectionId)).sort(cmp), target: { kind: 'slot', listId, sectionId: null }, ordered: manual },
        ...sections.map((section): BoardColumn => ({ key: `section:${section.id}`, title: section.name, tasks: inList.filter((t) => t.sectionId === section.id).sort(cmp), target: { kind: 'slot', listId, sectionId: section.id }, ordered: manual, section })),
      ];
    }
    case 'list': {
      // Within a list column: no-section cards first, then each section's, by position.
      const sectionRank = new Map(snap.sections.map((s) => [s.id, s.order + 1]));
      const listCmp = manual ? doneLast((a, b) => (sectionRank.get(a.sectionId ?? '') ?? 0) - (sectionRank.get(b.sectionId ?? '') ?? 0) || a.order - b.order) : cmp;
      return navOrder(snap.lists, snap.projectList)
        .map((list): BoardColumn => ({ key: `list:${list.id}`, title: list.name, tasks: cards.filter((t) => t.listId === list.id).sort(listCmp), target: { kind: 'slot', listId: list.id, sectionId: null, wholeList: true }, ordered: manual, list }))
        .filter((c) => c.list!.kind !== 'project' || c.list!.id === snap.projectList || c.tasks.length > 0);
    }
    case 'priority':
      return ([1, 2, 3, 4] as Priority[]).map((p) => ({ key: `p${p}`, title: `Priority ${p}`, tasks: cards.filter((t) => t.priority === p).sort(doneLast(byDue)), target: { kind: 'priority', priority: p }, ordered: false, priority: p }));
    case 'due': {
      const days = Array.from({ length: 7 }, (_, i) => addDays(ctx.today, i));
      const last = days[days.length - 1];
      const day = (t: TaskView) => dueDay(t);
      const sorted = (list: TaskView[]) => list.sort(doneLast(byDue));
      const out: BoardColumn[] = [];
      const overdue = cards.filter((t) => day(t) && day(t)! < ctx.today);
      if (overdue.length) out.push({ key: 'due:overdue', title: 'Overdue', tasks: sorted(overdue), target: { kind: 'none' }, ordered: false, tone: 'overdue' });
      for (const d of days) out.push({ key: `due:${d}`, title: dayHeading(d, ctx.today), tasks: sorted(cards.filter((t) => day(t) === d)), target: { kind: 'due', date: d }, ordered: false });
      const later = cards.filter((t) => day(t) && day(t)! > last);
      if (later.length) out.push({ key: 'due:later', title: 'Later', tasks: sorted(later), target: { kind: 'none' }, ordered: false });
      out.push({ key: 'due:', title: 'No date', tasks: cards.filter((t) => !day(t)).sort(cmp), target: { kind: 'due', date: '' }, ordered: false });
      return out;
    }
  }
}

export type BoardDrop = { kind: 'status'; status: string; before?: string; after?: string } | { kind: 'move'; target: MoveTarget } | { kind: 'update'; patch: TaskPatch };

/**
 * What dropping `moving` cards into `col` does: before/after the card `over` (ordered columns),
 * else into the column. `null` when nothing would change.
 */
export function boardDrop(tasks: Task[], col: BoardColumn, moving: string[], over?: { id: string; pos: 'before' | 'after' } | null): BoardDrop | null {
  const t = col.target;
  const movingTasks = tasks.filter((x) => moving.includes(x.id));
  if (!movingTasks.length) return null;
  switch (t.kind) {
    case 'status': {
      const here = (x: Task) => col.tasks.some((c) => c.id === x.id);
      // The done column: completes (positions don't matter there).
      if (t.done) return movingTasks.every((x) => x.done) ? null : { kind: 'status', status: t.status };
      const target = over && col.ordered ? col.tasks.find((x) => x.id === over.id && !moving.includes(x.id)) : undefined;
      if (target && over) return over.pos === 'before' ? { kind: 'status', status: t.status, before: target.id } : { kind: 'status', status: t.status, after: target.id };
      return movingTasks.every(here) ? null : { kind: 'status', status: t.status };
    }
    case 'slot': {
      const target = over && col.ordered ? tasks.find((x) => x.id === over.id && !moving.includes(x.id)) : undefined;
      if (target && over) return { kind: 'move', target: dropTarget(tasks, target, over.pos, moving) };
      // Dropped on a list column's empty space: already in that list → keep the section.
      if (t.wholeList && movingTasks.every((x) => x.listId === t.listId && !x.parentId)) return null;
      return { kind: 'move', target: { listId: t.listId, sectionId: t.sectionId, parentId: null, index: null } };
    }
    case 'priority':
      return movingTasks.every((x) => x.priority === t.priority) ? null : { kind: 'update', patch: { priority: t.priority } };
    case 'due':
      return movingTasks.every((x) => (dueDay(x) ?? '') === t.date) ? null : { kind: 'update', patch: { dueDate: t.date } };
    default:
      return null;
  }
}

/** Defaults for a task added from a view (quick add `Q`, AI suggestions, board columns): its list, Today's date, the label. */
export function viewDefaults(view: ViewRef, snap: Pick<Snapshot, 'today' | 'projectList'>): Omit<NewTask, 'text'> {
  switch (view.kind) {
    case 'list':
      return { listId: view.id };
    case 'inbox':
      return { listId: 'inbox' };
    case 'today':
      return { due: { date: snap.today }, listId: snap.projectList ?? 'inbox' };
    case 'label':
      return { labels: [view.name], listId: snap.projectList ?? 'inbox' };
    default:
      return { listId: snap.projectList ?? 'inbox' };
  }
}

/** Defaults for a task added at the bottom of a column: the view's (`base`) plus the column's. */
export function columnDefaults(col: BoardColumn, base: Omit<NewTask, 'text'>): Omit<NewTask, 'text'> {
  const t = col.target;
  switch (t.kind) {
    case 'status':
      return t.done ? base : { ...base, status: t.status };
    case 'slot':
      return { ...base, listId: t.listId, sectionId: t.sectionId ?? undefined };
    case 'priority':
      return { ...base, priority: t.priority };
    case 'due': {
      const { due: _drop, ...rest } = base;
      return t.date ? { ...rest, due: { date: t.date } } : rest;
    }
    default:
      return base;
  }
}
