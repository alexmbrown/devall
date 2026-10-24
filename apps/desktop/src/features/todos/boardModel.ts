/**
 * Pure helpers for the Board layout (Todoist-style columns): which columns a view gets, the cards
 * in each, and what dropping cards does (a `move` into a list/section, or an `update` of priority
 * or due date). No React — tested in `boardModel.test.ts`.
 */
import type { NewTask, Priority, Section, Snapshot, Task, TaskList, TaskPatch, TaskView } from '@/lib/api/todos';
import { addDays, attention, byDue, compareBy, dayHeading, dropTarget, dueDay, navOrder, type MoveTarget, type SortKey, type ViewRef } from './lib';

export type Layout = 'list' | 'board';
export type GroupBy = 'section' | 'list' | 'priority' | 'due';

export const GROUP_LABELS: Record<GroupBy, string> = { section: 'Section', list: 'List', priority: 'Priority', due: 'Due date' };

/** Board groupings that make sense for a view; the first is the default. */
export function groupChoices(view: ViewRef): GroupBy[] {
  switch (view.kind) {
    case 'list':
    case 'inbox':
      return ['section', 'priority', 'due'];
    case 'all':
      return ['list', 'priority', 'due'];
    case 'upcoming':
      return ['due', 'priority', 'list'];
    default:
      return ['priority', 'due', 'list'];
  }
}

/** Views that can be shown as a board (Completed is a log). */
export const canBoard = (view: ViewRef) => view.kind !== 'completed';

/** Where cards dropped into a column go. */
export type ColumnTarget =
  /** A list (`sectionId` null = no section). `wholeList`: the column shows every section of the list. */
  | { kind: 'slot'; listId: string; sectionId: string | null; wholeList?: boolean }
  | { kind: 'priority'; priority: Priority }
  /** `''` = no date. */
  | { kind: 'due'; date: string }
  /** Overdue / Later: nothing sensible to set. */
  | { kind: 'none' };

export interface BoardColumn {
  /** `section:<id>`, `section:` (no section), `list:<id>`, `p1`…`p4`, `due:<YYYY-MM-DD>`, `due:` (no date), `due:overdue`, `due:later`. */
  key: string;
  title: string;
  tasks: TaskView[];
  target: ColumnTarget;
  /** Cards keep their manual order: they can be dragged within the column. */
  ordered: boolean;
  /** For headers: the section or list the column stands for (rename, reorder). */
  section?: Section;
  list?: TaskList;
  priority?: Priority;
  tone?: 'overdue';
}

export interface BoardCtx {
  /** The list a list/Inbox view shows (section columns). */
  listId?: string;
  sort: SortKey;
  today: string;
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

/** Drops tasks whose parent is shown too: they appear as a count on the parent's card. */
export function topLevel(tasks: TaskView[]): TaskView[] {
  const ids = new Set(tasks.map((t) => t.id));
  return tasks.filter((t) => !t.parentId || !ids.has(t.parentId));
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
  const cards = topLevel(tasks);
  const manual = ctx.sort === 'manual';
  const cmp = doneLast(compareBy(ctx.sort));
  switch (groupBy) {
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

export type BoardDrop = { kind: 'move'; target: MoveTarget } | { kind: 'update'; patch: TaskPatch };

/**
 * What dropping `moving` cards into `col` does: before/after the card `over` (ordered columns),
 * else into the column. `null` when nothing would change.
 */
export function boardDrop(tasks: Task[], col: BoardColumn, moving: string[], over?: { id: string; pos: 'before' | 'after' } | null): BoardDrop | null {
  const t = col.target;
  const movingTasks = tasks.filter((x) => moving.includes(x.id));
  if (!movingTasks.length) return null;
  switch (t.kind) {
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

/** Defaults for a task added at the bottom of a column. */
export function columnDefaults(col: BoardColumn, fallbackList: string): Omit<NewTask, 'text'> {
  const t = col.target;
  switch (t.kind) {
    case 'slot':
      return { listId: t.listId, sectionId: t.sectionId ?? undefined };
    case 'priority':
      return { priority: t.priority, listId: fallbackList };
    case 'due':
      return t.date ? { due: { date: t.date }, listId: fallbackList } : { listId: fallbackList };
    default:
      return { listId: fallbackList };
  }
}
