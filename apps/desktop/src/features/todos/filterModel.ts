/**
 * The filter bar of to-do views (board and list layouts): free text plus facets (labels, priority,
 * due, status, list, "has…", a saved filter), matched instantly in the UI; and what a view hides
 * (completed tasks, folded sub-tasks…) for the notices under the bar. No React — tested in
 * `filterModel.test.ts`. Saved filters keep using the Rust filter language (`filterToQuery` turns
 * the bar into one).
 */
import type { Priority, Snapshot, StatusColumn, TaskView } from '@/lib/api/todos';
import { statusOf } from './boardModel';
import { addDays, dueDay, foldedUnder, type ViewRef } from './lib';

export type DueFacet = 'overdue' | 'today' | 'week' | 'none' | 'deadline';
export type HasFacet = 'attachments' | 'links' | 'subtasks' | 'comments' | 'description';

export interface TaskFilter {
  /** Free text: every word in the title, description, comments or labels. */
  q: string;
  labels: string[];
  /** Tasks with any of the labels, or with all of them. */
  labelMode: 'any' | 'all';
  priorities: Priority[];
  due: DueFacet[];
  /** Status column ids. */
  statuses: string[];
  /** List ids. */
  lists: string[];
  has: HasFacet[];
  /** A saved filter's id: only its tasks (saved filters match open tasks). */
  saved: string | null;
}

export const EMPTY_FILTER: TaskFilter = { q: '', labels: [], labelMode: 'any', priorities: [], due: [], statuses: [], lists: [], has: [], saved: null };

export const DUE_FACETS: { value: DueFacet; label: string }[] = [
  { value: 'overdue', label: 'Overdue' },
  { value: 'today', label: 'Due today' },
  { value: 'week', label: 'Next 7 days' },
  { value: 'none', label: 'No date' },
  { value: 'deadline', label: 'Has a deadline' },
];

export const HAS_FACETS: { value: HasFacet; label: string }[] = [
  { value: 'attachments', label: 'Attachments' },
  { value: 'links', label: 'Links' },
  { value: 'subtasks', label: 'Sub-tasks' },
  { value: 'comments', label: 'Comments' },
  { value: 'description', label: 'Description' },
];

/** A stored filter with every field (older or partial values fill in). */
export function normalizeFilter(f?: Partial<TaskFilter> | null): TaskFilter {
  const arr = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  return {
    q: typeof f?.q === 'string' ? f.q : '',
    labels: arr<string>(f?.labels),
    labelMode: f?.labelMode === 'all' ? 'all' : 'any',
    priorities: arr<Priority>(f?.priorities).filter((p) => [1, 2, 3, 4].includes(p)),
    due: arr<DueFacet>(f?.due),
    statuses: arr<string>(f?.statuses),
    lists: arr<string>(f?.lists),
    has: arr<HasFacet>(f?.has),
    saved: typeof f?.saved === 'string' ? f.saved : null,
  };
}

/** Facets in use (the text doesn't count). */
export function facetCount(f: TaskFilter): number {
  return [f.labels, f.priorities, f.due, f.statuses, f.lists, f.has].filter((x) => x.length > 0).length + (f.saved ? 1 : 0);
}

export function isFilterActive(f?: TaskFilter | null): boolean {
  return !!f && (!!f.q.trim() || facetCount(f) > 0);
}

/** What matching needs besides the filter. */
export interface FilterCtx {
  today: string;
  columns: StatusColumn[];
  /** Ids of tasks with sub-tasks. */
  parents: Set<string>;
  /** The saved filter's task ids (null: none chosen or it's gone). */
  saved: Set<string> | null;
}

export function filterCtx(snap: Pick<Snapshot, 'today' | 'settings' | 'tasks' | 'filters'>, f: TaskFilter): FilterCtx {
  const sf = f.saved ? snap.filters.find((x) => x.id === f.saved) : undefined;
  return {
    today: snap.today,
    columns: snap.settings.columns,
    parents: new Set(snap.tasks.flatMap((t) => (t.parentId ? [t.parentId] : []))),
    saved: sf ? new Set(sf.groups.flatMap((g) => g.ids)) : null,
  };
}

const words = (q: string) => q.toLowerCase().split(/\s+/).filter(Boolean);

/** Whether a task passes the filter: every facet in use (values of one facet: any of them). */
export function matchesFilter(t: TaskView, f: TaskFilter, cx: FilterCtx): boolean {
  const q = words(f.q);
  if (q.length) {
    const hay = [t.content, t.description ?? '', ...(t.labels ?? []).map((l) => `@${l}`), ...(t.comments ?? []).map((c) => c.text)].join('\n').toLowerCase();
    if (!q.every((w) => hay.includes(w))) return false;
  }
  if (f.labels.length) {
    const mine = new Set((t.labels ?? []).map((l) => l.toLowerCase()));
    const want = f.labels.map((l) => l.toLowerCase());
    if (f.labelMode === 'all' ? !want.every((l) => mine.has(l)) : !want.some((l) => mine.has(l))) return false;
  }
  if (f.priorities.length && !f.priorities.includes(t.priority)) return false;
  if (f.due.length) {
    const d = dueDay(t);
    const week = addDays(cx.today, 6);
    const hit = f.due.some((x) => (x === 'overdue' ? !!d && d < cx.today && !t.done : x === 'today' ? d === cx.today : x === 'week' ? !!d && d >= cx.today && d <= week : x === 'none' ? !d : !!t.deadline));
    if (!hit) return false;
  }
  if (f.statuses.length) {
    const c = statusOf(t, cx.columns);
    if (!c || !f.statuses.includes(c.id)) return false;
  }
  if (f.lists.length && !f.lists.includes(t.listId)) return false;
  if (f.has.length) {
    const has = (h: HasFacet) =>
      h === 'attachments' ? !!t.attachments?.length : h === 'links' ? !!t.links?.length : h === 'comments' ? !!t.comments?.length : h === 'description' ? !!t.description?.trim() : cx.parents.has(t.id);
    if (!f.has.some(has)) return false;
  }
  if (f.saved && !cx.saved?.has(t.id)) return false;
  return true;
}

/** `tasks` that pass (all of them when the filter is empty). */
export function applyFilter(tasks: TaskView[], f: TaskFilter | null | undefined, cx: FilterCtx): TaskView[] {
  return f && isFilterActive(f) ? tasks.filter((t) => matchesFilter(t, f, cx)) : tasks;
}

/** The filter language escapes `& | ( ) , ! \` with a backslash. */
const esc = (s: string) => s.replace(/[&|(),!\\]/g, (c) => `\\${c}`);
const anyOf = (terms: string[]) => (terms.length > 1 ? `(${terms.join(' | ')})` : terms[0]);

/**
 * The bar as a saved-filter query (`(@bug | @api) & p1 & overdue`), or null when nothing is set.
 * Saved filters only ever match open tasks.
 */
export function filterToQuery(f: TaskFilter, snap: Pick<Snapshot, 'lists' | 'settings' | 'filters'>): string | null {
  const parts: string[] = [];
  for (const w of words(f.q)) parts.push(`search: ${esc(w)}`);
  if (f.labels.length) parts.push(f.labelMode === 'all' ? f.labels.map((l) => `@${esc(l)}`).join(' & ') : anyOf(f.labels.map((l) => `@${esc(l)}`)));
  if (f.priorities.length) parts.push(anyOf([...f.priorities].sort().map((p) => `p${p}`)));
  if (f.due.length) parts.push(anyOf(f.due.map((d) => ({ overdue: 'overdue', today: 'today', week: '7 days', none: 'no date', deadline: 'has: deadline' })[d])));
  const cols = f.statuses.map((id) => snap.settings.columns.find((c) => c.id === id)?.name).filter((n): n is string => !!n);
  if (cols.length) parts.push(anyOf(cols.map((n) => `status: ${esc(n)}`)));
  const lists = f.lists.map((id) => snap.lists.find((l) => l.id === id)?.name).filter((n): n is string => !!n);
  if (lists.length) parts.push(anyOf(lists.map((n) => `#${esc(n)}`)));
  if (f.has.length) parts.push(anyOf(f.has.map((h) => `has: ${h}`)));
  const saved = f.saved ? snap.filters.find((x) => x.id === f.saved)?.query.trim() : undefined;
  // Groups (`a, b`) can't be nested in an expression.
  if (saved && !saved.includes(',')) parts.push(`(${saved})`);
  return parts.length ? parts.join(' & ') : null;
}

/** Short words for the active facets ("@bug", "p1", "overdue", "In progress"…), for notices and titles. */
export function describeFilter(f: TaskFilter, snap: Pick<Snapshot, 'lists' | 'settings' | 'filters'>): string[] {
  const out: string[] = [];
  if (f.q.trim()) out.push(`“${f.q.trim()}”`);
  if (f.labels.length) out.push(f.labels.map((l) => `@${l}`).join(f.labelMode === 'all' ? ' & ' : ' or '));
  if (f.priorities.length) out.push([...f.priorities].sort().map((p) => `p${p}`).join(' or '));
  if (f.due.length) out.push(f.due.map((d) => DUE_FACETS.find((x) => x.value === d)?.label.toLowerCase() ?? d).join(' or '));
  for (const id of f.statuses) out.push(snap.settings.columns.find((c) => c.id === id)?.name ?? id);
  for (const id of f.lists) out.push(`#${snap.lists.find((l) => l.id === id)?.name ?? id}`);
  if (f.has.length) out.push(`with ${f.has.map((h) => HAS_FACETS.find((x) => x.value === h)?.label.toLowerCase() ?? h).join(' or ')}`);
  if (f.saved) out.push(snap.filters.find((x) => x.id === f.saved)?.name ?? 'saved filter');
  return out;
}

// ── what a view hides ────────────────────────────────────────────────────────────────────────────

/** Something a view leaves out, shown as a notice with an action ("3 completed hidden · Show"). */
export type Hidden =
  /** Completed tasks (in the snapshot) not shown: list layout with "show completed" off. */
  | { kind: 'done'; count: number }
  /** Completed tasks older than a week that the snapshot doesn't carry ("Load"). */
  | { kind: 'older'; count: number }
  /** Sub-tasks under collapsed parents (list layout); `parents` = what "Expand" opens. */
  | { kind: 'folded'; count: number; parents: string[] }
  /** Sub-tasks inside their parent's card (board, nested sub-tasks). */
  | { kind: 'nested'; count: number }
  /** Completed tasks before the done column's window (status board). */
  | { kind: 'window'; count: number; days: number };

/** Lists whose tasks a view shows in full (completed ones included): list views and All tasks. */
export function viewLists(view: ViewRef): 'all' | string[] | null {
  switch (view.kind) {
    case 'all':
      return 'all';
    case 'inbox':
      return ['inbox'];
    case 'list':
      return [view.id];
    default:
      return null;
  }
}

/** Completed tasks of the view's lists that the snapshot left out (older than a week). */
export function olderDoneIn(snap: Pick<Snapshot, 'olderDone'>, view: ViewRef): number {
  const lists = viewLists(view);
  const older = snap.olderDone ?? {};
  if (!lists) return 0;
  return lists === 'all' ? Object.values(older).reduce((a, n) => a + n, 0) : lists.reduce((a, id) => a + (older[id] ?? 0), 0);
}

/** What the current view shows and hides (the filter bar's "n of m" and the notices under it). */
export interface ViewStats {
  /** Cards / tasks shown (after the filter). */
  shown: number;
  /** Shown without the filter. */
  total: number;
  unit: 'cards' | 'tasks';
  hidden: Hidden[];
}

/**
 * Counts of a list-layout view over `tasks` (every task of its lists in the snapshot, completed
 * ones included): with completed tasks hidden, they (and the older ones not loaded, `older`) are
 * one "Show"; shown, the older ones are a "Load"; sub-tasks under collapsed parents (unless a
 * filter opens them) are an "Expand".
 */
export function listStats(tasks: TaskView[], showDone: boolean, match: ((t: TaskView) => boolean) | undefined, older: number): ViewStats {
  const scope = showDone ? tasks : tasks.filter((t) => !t.done);
  const done = tasks.length - tasks.filter((t) => !t.done).length;
  const hidden: Hidden[] = [showDone ? { kind: 'older', count: older } : { kind: 'done', count: done + older }];
  if (!match) hidden.push({ kind: 'folded', ...foldedUnder(tasks, showDone) });
  return { shown: match ? scope.filter(match).length : scope.length, total: scope.length, unit: 'tasks', hidden };
}

/** Notices worth showing, in a fixed order, without empty ones. */
export function hiddenNotices(h: Hidden[]): Hidden[] {
  const order: Hidden['kind'][] = ['done', 'older', 'window', 'folded', 'nested'];
  return h.filter((x) => x.count > 0).sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
}

/** A notice's text. */
export function hiddenText(h: Hidden): string {
  const n = h.count;
  const s = n === 1 ? '' : 's';
  switch (h.kind) {
    case 'done':
      return `${n} completed task${s} hidden`;
    case 'older':
      return `${n} task${s} completed more than a week ago not loaded`;
    case 'folded':
      return `${n} sub-task${s} folded under collapsed tasks`;
    case 'nested':
      return `${n} sub-task${s} shown inside ${n === 1 ? 'its parent card' : 'parent cards'}`;
    case 'window':
      return `${n} completed before the last ${h.days === 1 ? 'day' : `${h.days} days`} not in the done column`;
  }
}
