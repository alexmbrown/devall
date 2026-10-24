/**
 * Pure helpers for the to-do UI: dates, view grouping (Today, Upcoming, lists), the task tree,
 * drag-and-drop targets, keyboard shortcuts and quick-add highlighting. No React, no IPC — tested in
 * `lib.test.ts`. Natural-language parsing and filters live in Rust (`devall_core::todos`).
 */
import type { Due, Priority, QuickToken, Section, Snapshot, Task, TaskList, TaskView } from '@/lib/api/todos';

export type ViewRef =
  | { kind: 'inbox' }
  | { kind: 'today' }
  | { kind: 'upcoming' }
  | { kind: 'completed' }
  | { kind: 'all' }
  | { kind: 'list'; id: string }
  | { kind: 'label'; name: string }
  | { kind: 'filter'; id: string };

export function viewKey(v: ViewRef): string {
  switch (v.kind) {
    case 'list':
      return `list:${v.id}`;
    case 'label':
      return `label:${v.name}`;
    case 'filter':
      return `filter:${v.id}`;
    default:
      return v.kind;
  }
}

export function parseViewKey(key: string): ViewRef | null {
  const [kind, ...rest] = key.split(':');
  const arg = rest.join(':');
  switch (kind) {
    case 'inbox':
    case 'today':
    case 'upcoming':
    case 'completed':
    case 'all':
      return { kind };
    case 'list':
      return arg ? { kind, id: arg } : null;
    case 'label':
      return arg ? { kind, name: arg } : null;
    case 'filter':
      return arg ? { kind, id: arg } : null;
    default:
      return null;
  }
}

// ── dates (local calendar days as YYYY-MM-DD) ───────────────────────────────────────────────────

const pad = (n: number) => String(n).padStart(2, '0');

export function ymd(d: Date): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

export function parseYmd(s: string): Date {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, (m || 1) - 1, d || 1);
}

export function addDays(s: string, n: number): string {
  const d = parseYmd(s);
  d.setDate(d.getDate() + n);
  return ymd(d);
}

/** Whole days from `a` to `b` (DST-safe). */
export function daysBetween(a: string, b: string): number {
  return Math.round((parseYmd(b).getTime() - parseYmd(a).getTime()) / 86_400_000);
}

/** Monday of the week containing `s`. */
export function startOfWeek(s: string): string {
  const d = parseYmd(s);
  return addDays(s, -((d.getDay() + 6) % 7));
}

export function weekDays(start: string): string[] {
  return Array.from({ length: 7 }, (_, i) => addDays(start, i));
}

/** The 6×7 grid of days shown for the month containing `s` (weeks start on Monday). */
export function monthGrid(s: string): string[] {
  const first = `${s.slice(0, 7)}-01`;
  const start = startOfWeek(first);
  return Array.from({ length: 42 }, (_, i) => addDays(start, i));
}

export function todayYmd(): string {
  return ymd(new Date());
}

/** The local day a task is due. */
export const dueDay = (t: { due?: Due; dueLocal?: string }) => t.dueLocal ?? t.due?.date;

export type DueTone = 'overdue' | 'today' | 'tomorrow' | 'soon' | 'later';

const fmt = (s: string, opts: Intl.DateTimeFormatOptions, locale?: string) => parseYmd(s).toLocaleDateString(locale, opts);

/** Short label for a day relative to `today`: Today, Tomorrow, Yesterday, Mon, 30 Oct, 3 Jan 2027. */
export function dayLabel(s: string, today: string, locale?: string): string {
  const n = daysBetween(today, s);
  if (n === 0) return 'Today';
  if (n === 1) return 'Tomorrow';
  if (n === -1) return 'Yesterday';
  if (n > 1 && n < 7) return fmt(s, { weekday: 'short' }, locale);
  if (s.slice(0, 4) === today.slice(0, 4)) return fmt(s, { day: 'numeric', month: 'short' }, locale);
  return fmt(s, { day: 'numeric', month: 'short', year: 'numeric' }, locale);
}

/** Group header for a day: "Today · Fri 23 Oct", "Mon 26 Oct". */
export function dayHeading(s: string, today: string, locale?: string): string {
  const full = fmt(s, { weekday: 'short', day: 'numeric', month: 'short' }, locale);
  const n = daysBetween(today, s);
  if (n === 0) return `Today · ${full}`;
  if (n === 1) return `Tomorrow · ${full}`;
  return full;
}

export function toneOf(s: string, today: string): DueTone {
  const n = daysBetween(today, s);
  return n < 0 ? 'overdue' : n === 0 ? 'today' : n === 1 ? 'tomorrow' : n < 7 ? 'soon' : 'later';
}

/** `HH:MM` → locale time (17:00 / 5:00 PM). */
export function timeLabel(hm: string, locale?: string): string {
  const [h, m] = hm.split(':').map(Number);
  return new Date(2000, 0, 1, h, m).toLocaleTimeString(locale, { hour: 'numeric', minute: '2-digit' });
}

export interface DueInfo {
  text: string;
  tone: DueTone;
  time?: string;
  recurring: boolean;
  /** Timed task whose time already passed today. */
  late: boolean;
}

export function dueInfo(t: { due?: Due; dueLocal?: string; dueAt?: number }, today: string, now = Date.now(), locale?: string): DueInfo | null {
  const day = dueDay(t);
  if (!t.due || !day) return null;
  const time = t.dueAt ? new Date(t.dueAt).toLocaleTimeString(locale, { hour: 'numeric', minute: '2-digit' }) : t.due.time ? timeLabel(t.due.time, locale) : undefined;
  const tone = toneOf(day, today);
  return { text: dayLabel(day, today, locale), tone, time, recurring: !!t.due.recurrence, late: tone === 'today' && !!t.dueAt && t.dueAt < now };
}

/** A recurring rule as text (the typed rule when we have it). */
export function ruleText(due?: Due): string {
  if (!due?.recurrence) return '';
  if (due.string) return due.string;
  const r = due.recurrence;
  const n = r.interval && r.interval > 1 ? `${r.interval} ` : '';
  const unit = { day: 'day', week: 'week', month: 'month', year: 'year' }[r.freq];
  const on = r.weekdays?.length ? ` on ${r.weekdays.join(', ')}` : r.monthDays?.length ? ` on the ${r.monthDays.map((d) => (d < 0 ? 'last day' : `${d}.`)).join(', ')}` : '';
  return `every${r.fromCompletion ? '!' : ''} ${n}${unit}${n ? 's' : ''}${on}`;
}

// ── views ───────────────────────────────────────────────────────────────────────────────────────

const timeKey = (t: TaskView) => t.dueAt ?? (t.due?.time ? Number(t.due.time.replace(':', '')) : Number.MAX_SAFE_INTEGER);

/** Due day (undated last), timed before untimed, then priority and position. */
export function byDue(a: TaskView, b: TaskView): number {
  const da = dueDay(a) ?? '9999-99-99';
  const db = dueDay(b) ?? '9999-99-99';
  if (da !== db) return da < db ? -1 : 1;
  const ta = timeKey(a);
  const tb = timeKey(b);
  if (ta !== tb) return ta - tb;
  if (a.priority !== b.priority) return a.priority - b.priority;
  return a.order - b.order;
}

/** Open tasks due today or earlier — the Today view and the sidebar badge. */
export function attention(tasks: TaskView[], today: string): TaskView[] {
  return tasks.filter((t) => !t.done && (dueDay(t) ?? '9999') <= today);
}

export function todayGroups(tasks: TaskView[], today: string): { overdue: TaskView[]; today: TaskView[] } {
  const open = attention(tasks, today).sort(byDue);
  return { overdue: open.filter((t) => dueDay(t)! < today), today: open.filter((t) => dueDay(t) === today) };
}

/** Upcoming: overdue, then each day from `today` for `days` days. */
export function upcomingDays(tasks: TaskView[], today: string, days: number): { overdue: TaskView[]; days: { date: string; tasks: TaskView[] }[] } {
  const end = addDays(today, days - 1);
  const open = tasks.filter((t) => !t.done && dueDay(t)).sort(byDue);
  const byDay = new Map<string, TaskView[]>();
  for (const t of open) {
    const d = dueDay(t)!;
    if (d >= today && d <= end) byDay.set(d, [...(byDay.get(d) ?? []), t]);
  }
  return {
    overdue: open.filter((t) => dueDay(t)! < today),
    days: Array.from({ length: days }, (_, i) => addDays(today, i)).map((date) => ({ date, tasks: byDay.get(date) ?? [] })),
  };
}

export interface Row {
  task: TaskView;
  /** Visual depth (clamped to `MAX_DEPTH`). */
  depth: number;
  /** Open sub-tasks. */
  open: number;
  /** All sub-tasks. */
  kids: number;
}

/** Sub-task nesting shown in lists (deeper tasks render at this depth). */
export const MAX_DEPTH = 4;

/** How list-like views order tasks: by position (drag to reorder) or by an attribute. */
export type SortKey = 'manual' | 'due' | 'priority' | 'added' | 'name';

export const SORTS: { value: SortKey; label: string }[] = [
  { value: 'manual', label: 'Manual' },
  { value: 'due', label: 'Due date' },
  { value: 'priority', label: 'Priority' },
  { value: 'added', label: 'Date added' },
  { value: 'name', label: 'Name' },
];

/** Comparator for `sort` (ties by position). */
export function compareBy(sort: SortKey): (a: TaskView, b: TaskView) => number {
  switch (sort) {
    case 'due':
      return byDue;
    case 'priority':
      return (a, b) => a.priority - b.priority || byDue(a, b);
    case 'added':
      return (a, b) => b.createdAt - a.createdAt || a.order - b.order;
    case 'name':
      return (a, b) => a.content.localeCompare(b.content, undefined, { sensitivity: 'base', numeric: true }) || a.order - b.order;
    default:
      return (a, b) => a.order - b.order;
  }
}

/**
 * Tasks of one slot (list + section) as tree rows: sub-tasks after their parent (unless the
 * parent is collapsed), siblings by position (or `sort`), completed ones last (only with `showDone`).
 */
export function treeRows(tasks: TaskView[], showDone: boolean, maxDepth = MAX_DEPTH, sort: SortKey = 'manual'): Row[] {
  const ids = new Set(tasks.map((t) => t.id));
  const kids = new Map<string, TaskView[]>();
  for (const t of tasks) {
    const p = t.parentId && ids.has(t.parentId) ? t.parentId : '';
    kids.set(p, [...(kids.get(p) ?? []), t]);
  }
  const cmp = compareBy(sort);
  const sorted = (list: TaskView[]) => [...list].sort((a, b) => Number(!!a.done) - Number(!!b.done) || cmp(a, b));
  const out: Row[] = [];
  const walk = (parent: string, depth: number, seen: Set<string>) => {
    for (const t of sorted(kids.get(parent) ?? [])) {
      if ((t.done && !showDone) || seen.has(t.id)) continue;
      const children = kids.get(t.id) ?? [];
      out.push({ task: t, depth: Math.min(depth, maxDepth), open: children.filter((c) => !c.done).length, kids: children.length });
      if (!t.collapsed) walk(t.id, depth + 1, new Set([...seen, t.id]));
    }
  };
  walk('', 0, new Set());
  return out;
}

export interface ListLayout {
  rows: Row[];
  sections: { section: Section; rows: Row[]; open: number }[];
}

export function listLayout(snap: Snapshot, listId: string, showDone: boolean, sort: SortKey = 'manual'): ListLayout {
  const inList = snap.tasks.filter((t) => t.listId === listId);
  const sections = snap.sections.filter((s) => s.listId === listId).sort((a, b) => a.order - b.order);
  const known = new Set(sections.map((s) => s.id));
  return {
    rows: treeRows(
      inList.filter((t) => !t.sectionId || !known.has(t.sectionId)),
      showDone,
      MAX_DEPTH,
      sort,
    ),
    sections: sections.map((section) => {
      const ts = inList.filter((t) => t.sectionId === section.id);
      return { section, rows: treeRows(ts, showDone, MAX_DEPTH, sort), open: ts.filter((t) => !t.done).length };
    }),
  };
}

/** Lists in the order the left pane shows them: Inbox, the open project's, the user's, other projects. */
export function navOrder(lists: TaskList[], projectList?: string | null): TaskList[] {
  const rank = (l: TaskList) => (l.kind === 'inbox' ? 0 : l.id === projectList ? 1 : l.kind === 'user' ? 2 : 3);
  return [...lists].sort((a, b) => rank(a) - rank(b) || a.order - b.order);
}

/** "All tasks": every list with tasks to show, in the left pane's order, each laid out as a list (sections flattened). */
export function allLayout(snap: Snapshot, showDone: boolean, sort: SortKey = 'manual'): { list: TaskList; rows: Row[]; open: number }[] {
  return navOrder(snap.lists, snap.projectList)
    .map((list) => {
      const l = listLayout(snap, list.id, showDone, sort);
      return { list, rows: [...l.rows, ...l.sections.flatMap((s) => s.rows)], open: openCount(snap.tasks, (t) => t.listId === list.id) };
    })
    .filter((g) => g.rows.length > 0);
}

export function openCount(tasks: TaskView[], pred: (t: TaskView) => boolean): number {
  let n = 0;
  for (const t of tasks) if (!t.done && pred(t)) n++;
  return n;
}

/** Every word must appear in the content, description, labels or comments. */
export function searchTasks(tasks: TaskView[], q: string): TaskView[] {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return tasks.filter((t) => {
    const hay = [t.content, t.description ?? '', ...(t.labels ?? []).map((l) => `@${l}`), ...(t.comments ?? []).map((c) => c.text)].join('\n').toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** `id` and all its descendants (from the snapshot's tasks). */
export function subtree(tasks: Task[], id: string): string[] {
  const out = [id];
  for (let i = 0; i < out.length; i++) for (const t of tasks) if (t.parentId === out[i] && !out.includes(t.id)) out.push(t.id);
  return out;
}

// ── moving ──────────────────────────────────────────────────────────────────────────────────────

export interface MoveTarget {
  listId: string;
  sectionId: string | null;
  parentId: string | null;
  index: number | null;
}

const sameSlot = (a: Task, b: Task) => a.listId === b.listId && (a.sectionId ?? null) === (b.sectionId ?? null) && (a.parentId ?? null) === (b.parentId ?? null);

/** Open siblings of `t` (same list, section and parent) by position. */
export function siblings(tasks: Task[], t: Task, exclude: string[] = []): Task[] {
  return tasks.filter((x) => !x.done && sameSlot(x, t) && !exclude.includes(x.id)).sort((a, b) => a.order - b.order);
}

export type DropPos = 'before' | 'after' | 'child';

/** Where a dragged row lands on `rect`: top half → before; bottom half → after, or as a sub-task when the pointer is right of the row's indent. */
export function dropPosition(rect: { top: number; height: number; left: number }, x: number, y: number, depth: number, indent = 24): DropPos {
  if (y < rect.top + rect.height / 2) return 'before';
  return x > rect.left + (depth + 1) * indent + 36 && depth < MAX_DEPTH ? 'child' : 'after';
}

/** The `move` target for dropping `moving` tasks relative to `target`. */
export function dropTarget(tasks: Task[], target: Task, pos: DropPos, moving: string[]): MoveTarget {
  if (pos === 'child') return { listId: target.listId, sectionId: target.sectionId ?? null, parentId: target.id, index: null };
  const sibs = siblings(tasks, target, moving);
  const at = sibs.findIndex((s) => s.id === target.id);
  return { listId: target.listId, sectionId: target.sectionId ?? null, parentId: target.parentId ?? null, index: Math.max(0, at) + (pos === 'after' ? 1 : 0) };
}

/** Indent: become the last sub-task of the sibling above. */
export function indentTarget(tasks: Task[], t: Task): MoveTarget | null {
  const sibs = siblings(tasks, t);
  const i = sibs.findIndex((s) => s.id === t.id);
  if (i <= 0) return null;
  return { listId: t.listId, sectionId: t.sectionId ?? null, parentId: sibs[i - 1].id, index: null };
}

/** `ids` with `moving` placed before/after `target` (reordering lists, sections, filters). */
export function reorderIds(ids: string[], moving: string, target: string, pos: 'before' | 'after'): string[] {
  if (moving === target || !ids.includes(moving)) return ids;
  const rest = ids.filter((x) => x !== moving);
  const at = rest.indexOf(target);
  if (at < 0) return ids;
  rest.splice(at + (pos === 'after' ? 1 : 0), 0, moving);
  return rest;
}

/** Top or bottom half of a row/header under the pointer. */
export function halfOf(rect: { top: number; height: number }, y: number): 'before' | 'after' {
  return y < rect.top + rect.height / 2 ? 'before' : 'after';
}

/** Outdent: become the sibling right after the parent. */
export function outdentTarget(tasks: Task[], t: Task): MoveTarget | null {
  const parent = t.parentId ? tasks.find((x) => x.id === t.parentId) : undefined;
  if (!parent) return null;
  const sibs = siblings(tasks, parent);
  const i = sibs.findIndex((s) => s.id === parent.id);
  return { listId: parent.listId, sectionId: parent.sectionId ?? null, parentId: parent.parentId ?? null, index: i + 1 };
}

// ── keyboard ────────────────────────────────────────────────────────────────────────────────────

export type KeyAction =
  | 'quickAdd'
  | 'search'
  | 'down'
  | 'up'
  | 'edit'
  | 'open'
  | 'complete'
  | 'p1'
  | 'p2'
  | 'p3'
  | 'p4'
  | 'today'
  | 'tomorrow'
  | 'noDate'
  | 'indent'
  | 'outdent'
  | 'delete'
  | 'selectAll'
  | 'escape';

/** The list shortcut for a key press (outside text fields). */
export function keyAction(e: { key: string; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean; altKey?: boolean }): KeyAction | null {
  const mod = !!(e.ctrlKey || e.metaKey);
  if (e.altKey) return null;
  if (mod) {
    if (e.key === ']') return 'indent';
    if (e.key === '[') return 'outdent';
    if (e.key === 'Enter') return 'complete';
    if (e.key.toLowerCase() === 'a') return 'selectAll';
    return null;
  }
  switch (e.key) {
    case 'q':
      return 'quickAdd';
    case '/':
      return 'search';
    case 'j':
    case 'ArrowDown':
      return 'down';
    case 'k':
    case 'ArrowUp':
      return 'up';
    case 'Enter':
      return e.shiftKey ? 'open' : 'edit';
    case 'e':
      return 'edit';
    case 'o':
      return 'open';
    case ' ':
      return 'complete';
    case '1':
    case '2':
    case '3':
    case '4':
      return `p${e.key}` as KeyAction;
    case 't':
      return 'today';
    case 'm':
      return 'tomorrow';
    case 'n':
      return 'noDate';
    case 'Delete':
    case 'Backspace':
      return 'delete';
    case 'Escape':
      return 'escape';
    default:
      return null;
  }
}

/** Next cursor position in `order` (wraps at neither end). */
export function moveCursor(order: string[], cursor: string | null, dir: 1 | -1): string | null {
  if (!order.length) return null;
  const i = cursor ? order.indexOf(cursor) : -1;
  if (i < 0) return dir === 1 ? order[0] : order[order.length - 1];
  return order[Math.min(order.length - 1, Math.max(0, i + dir))];
}

/** Shift-click range between `anchor` and `id` in display order. */
export function rangeSelect(order: string[], anchor: string | null, id: string): string[] {
  const a = anchor ? order.indexOf(anchor) : -1;
  const b = order.indexOf(id);
  if (a < 0 || b < 0) return [id];
  return order.slice(Math.min(a, b), Math.max(a, b) + 1);
}

// ── quick add highlighting ──────────────────────────────────────────────────────────────────────

export interface Segment {
  text: string;
  token?: QuickToken;
}

/** Splits `text` at token spans (UTF-16 offsets, as the parser reports them). */
export function segments(text: string, tokens: QuickToken[]): Segment[] {
  const out: Segment[] = [];
  let at = 0;
  for (const t of [...tokens].sort((a, b) => a.start - b.start)) {
    if (t.start < at || t.end > text.length || text.slice(t.start, t.end) !== t.text) continue;
    if (t.start > at) out.push({ text: text.slice(at, t.start) });
    out.push({ text: text.slice(t.start, t.end), token: t });
    at = t.end;
  }
  if (at < text.length) out.push({ text: text.slice(at) });
  return out;
}

/** The `#list` / `@label` / `/section` being typed at the caret, for autocomplete. */
export function sigilAt(text: string, caret: number): { sigil: '#' | '@' | '/'; start: number; partial: string } | null {
  const m = /(^|\s)([#@/])([^\s#@/"{}]*)$/.exec(text.slice(0, caret));
  if (!m) return null;
  return { sigil: m[2] as '#' | '@' | '/', start: m.index + m[1].length, partial: m[3] };
}

/** Names starting with (then containing) `partial`, case-insensitive. */
export function suggest(names: string[], partial: string, limit = 6): string[] {
  const p = partial.toLowerCase();
  const starts = names.filter((n) => n.toLowerCase().startsWith(p));
  const contains = names.filter((n) => !n.toLowerCase().startsWith(p) && n.toLowerCase().includes(p));
  return [...starts, ...contains].filter((n) => n.toLowerCase() !== p || p === '').slice(0, limit);
}

// ── colours & priorities ────────────────────────────────────────────────────────────────────────

export const COLORS = ['red', 'orange', 'yellow', 'green', 'teal', 'blue', 'purple', 'pink', 'gray'] as const;
export type ColorName = (typeof COLORS)[number];

/** CSS colour for a palette name (themed tokens, see todos.css). */
export function colorVar(c?: string | null): string | undefined {
  return c && (COLORS as readonly string[]).includes(c) ? `var(--td-${c})` : undefined;
}

export const PRIORITIES: { value: Priority; label: string }[] = [
  { value: 1, label: 'Priority 1' },
  { value: 2, label: 'Priority 2' },
  { value: 3, label: 'Priority 3' },
  { value: 4, label: 'Priority 4' },
];

/** Link label: `http/users/list.req.yaml` → `users/list`. */
export function linkLabel(l: string): string {
  return l
    .replace(/^https?:\/\//, '')
    .replace(/^http\//, '')
    .replace(/\.(req|ws|grpc|mqtt|sio)\.yaml$/, '')
    .replace(/^notes\//, '')
    .replace(/\.md$/, '');
}

/** Top-level folders of a /dev/all project (see docs/DEVELOPING.md). */
const PROJECT_FOLDERS = ['http', 'environments', 'mocks', 'plugins', 'openapi', 'load', 'data', 'webhooks', 'connections', 'queries', 'notes', 'utilities', 'devall.yaml'];

/** An absolute file-system path: `C:\…`, `C:/…`, `\\server\share`, `/…`. */
export function isAbsolutePath(p: string): boolean {
  return /^[a-zA-Z]:[\\/]/.test(p) || /^\\\\/.test(p) || p.startsWith('/');
}

/** `root` + a `/`-separated relative path, with the root's own separator (Windows or POSIX). */
export function joinPath(root: string, rel: string): string {
  const sep = root.includes('\\') ? '\\' : '/';
  const parts = rel.split('/').filter((s) => s && s !== '.');
  return [root.replace(/[\\/]+$/, ''), ...parts].join(sep);
}

/** The /dev/all view that opens a project document (requests, notes). */
export function appDocument(rel: string): { section: 'http' | 'notes'; param: string } | undefined {
  const req = /^http\/(.+\.(?:req|ws|grpc|mqtt|sio)\.yaml)$/.exec(rel);
  if (req) return { section: 'http', param: req[1] };
  const note = /^notes\/(.+\.(?:md|ipynb))$/.exec(rel);
  if (note) return { section: 'notes', param: note[1] };
  return undefined;
}

export type LinkTarget =
  /** Opens in the default browser / mail app. */
  | { kind: 'url'; url: string }
  /** A file to reveal; `rel` when it is inside the task's project. */
  | { kind: 'file'; path: string; rel?: string }
  /** Project-relative, but no project folder is known for the task. */
  | { kind: 'unresolved'; rel: string };

/**
 * What a task link points at: a URL, an absolute path, or a path relative to the task's project
 * (`http/users/list.req.yaml`, `notes/x.md`, `/notes/x.md`) resolved against `projectRoot`.
 */
export function classifyLink(link: string, projectRoot?: string | null): LinkTarget {
  let p = link.trim();
  if (/^(https?:\/\/|mailto:)/i.test(p)) return { kind: 'url', url: p };
  if (/^file:\/\//i.test(p)) {
    try {
      p = decodeURI(p.replace(/^file:\/\/(localhost)?/i, ''));
    } catch {
      p = p.replace(/^file:\/\//i, '');
    }
    if (/^\/[a-zA-Z]:[\\/]/.test(p)) p = p.slice(1);
    return { kind: 'file', path: p };
  }
  const first = p.replace(/^[\\/]+/, '').split(/[\\/]/)[0];
  // `/notes/x.md` is project-relative (as in notes' links); `/Users/me/x.txt` is absolute.
  const projectRelative = !isAbsolutePath(p) || (/^\/[^/\\]/.test(p) && PROJECT_FOLDERS.includes(first));
  if (!projectRelative) return { kind: 'file', path: p };
  const rel = p.replace(/\\/g, '/').replace(/^(\.\/|\/)+/, '');
  return projectRoot ? { kind: 'file', path: joinPath(projectRoot, rel), rel } : { kind: 'unresolved', rel };
}
