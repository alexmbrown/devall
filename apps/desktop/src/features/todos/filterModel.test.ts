import { describe, expect, it } from 'vitest';
import type { Snapshot, StatusColumn, TaskView } from '@/lib/api/todos';
import { boardColumns, boardTasks } from './boardModel';
import {
  applyFilter,
  describeFilter,
  EMPTY_FILTER,
  facetCount,
  filterCtx,
  filterToQuery,
  hiddenNotices,
  hiddenText,
  isFilterActive,
  listStats,
  matchesFilter,
  normalizeFilter,
  olderDoneIn,
  type TaskFilter,
} from './filterModel';
import { allLayout } from './lib';
import { storedBoard } from './ui';

const TODAY = '2026-10-23';
const COLUMNS: StatusColumn[] = [
  { id: 'todo', name: 'To do' },
  { id: 'wip', name: 'In progress' },
  { id: 'done', name: 'Done', done: true },
];

let n = 0;
function task(p: Partial<TaskView> & { id?: string }): TaskView {
  n++;
  return { id: p.id ?? `t${n}`, content: p.content ?? `Task ${n}`, listId: 'L', order: n, priority: 4, createdAt: 0, updatedAt: 0, ...p };
}

function snapshot(tasks: TaskView[], extra: Partial<Snapshot> = {}): Snapshot {
  return {
    lists: [
      { id: 'inbox', name: 'Inbox', kind: 'inbox', order: -1 },
      { id: 'L', name: 'Work & play', kind: 'user', order: 1 },
      { id: 'P', name: 'Shop', kind: 'project', order: 2 },
    ],
    sections: [],
    labels: [{ name: 'bug', color: 'red' }, { name: 'api' }],
    filters: [
      { id: 'F', name: 'Urgent', query: 'p1 | overdue', order: 0, count: 2, groups: [{ title: 'p1 | overdue', ids: ['a', 'c'] }] },
      { id: 'G', name: 'Split', query: 'p1, p2', order: 1, count: 0, groups: [] },
    ],
    settings: { reminders: true, remindBeforeMinutes: 0, columns: COLUMNS },
    tasks,
    projectList: 'P',
    completedCount: 0,
    olderDone: {},
    today: TODAY,
    ...extra,
  };
}

const tasks = [
  task({ id: 'a', content: 'Fix refund flow', priority: 1, labels: ['bug', 'api'], due: { date: TODAY }, status: 'wip' }),
  task({ id: 'b', content: 'Docs', description: 'mention the refund limits', labels: ['api'], attachments: [{ id: 'x', name: 'x.png', mime: 'image/png', size: 1, addedAt: 0 }] }),
  task({ id: 'c', content: 'Old thing', priority: 2, due: { date: '2026-10-20' }, links: ['https://x'], comments: [{ id: 'c1', text: 'Asked about REFUNDS', at: 0 }] }),
  task({ id: 'd', content: 'Someday', listId: 'P', deadline: '2026-11-01' }),
  task({ id: 'a1', content: 'Sub of a', parentId: 'a', due: { date: '2026-10-27' } }),
  task({ id: 'z', content: 'Shipped', done: true, completedAt: 1, labels: ['bug'] }),
];
const snap = snapshot(tasks);
const ids = (f: Partial<TaskFilter>) => applyFilter(tasks, { ...EMPTY_FILTER, ...f }, filterCtx(snap, { ...EMPTY_FILTER, ...f })).map((t) => t.id);

describe('filter bar matching', () => {
  it('is empty until something is set', () => {
    expect(isFilterActive(EMPTY_FILTER)).toBe(false);
    expect(isFilterActive({ ...EMPTY_FILTER, q: '  ' })).toBe(false);
    expect(isFilterActive({ ...EMPTY_FILTER, q: 'x' })).toBe(true);
    expect(isFilterActive({ ...EMPTY_FILTER, due: ['none'] })).toBe(true);
    expect(facetCount({ ...EMPTY_FILTER, labels: ['a', 'b'], priorities: [1], saved: 'F' })).toBe(3);
    expect(ids({})).toEqual(tasks.map((t) => t.id));
  });

  it('matches every word in the title, description, comments and labels', () => {
    expect(ids({ q: 'refund' })).toEqual(['a', 'b', 'c']);
    expect(ids({ q: 'REFUND @api' })).toEqual(['a', 'b']);
    expect(ids({ q: 'nothing-like-it' })).toEqual([]);
  });

  it('matches labels (any or all), priority, due, status, list and "has"', () => {
    expect(ids({ labels: ['bug', 'api'] })).toEqual(['a', 'b', 'z']);
    expect(ids({ labels: ['bug', 'API'], labelMode: 'all' })).toEqual(['a']);
    expect(ids({ priorities: [1, 2] })).toEqual(['a', 'c']);
    expect(ids({ due: ['overdue'] })).toEqual(['c']);
    expect(ids({ due: ['today', 'none'] })).toEqual(['a', 'b', 'd', 'z']);
    expect(ids({ due: ['week'] })).toEqual(['a', 'a1']);
    expect(ids({ due: ['deadline'] })).toEqual(['d']);
    // The done column holds the completed ones; unknown statuses are the first column.
    expect(ids({ statuses: ['wip'] })).toEqual(['a']);
    expect(ids({ statuses: ['done'] })).toEqual(['z']);
    expect(ids({ statuses: ['todo'] })).toEqual(['b', 'c', 'd', 'a1']);
    expect(ids({ lists: ['P'] })).toEqual(['d']);
    expect(ids({ has: ['attachments', 'links'] })).toEqual(['b', 'c']);
    expect(ids({ has: ['subtasks'] })).toEqual(['a']);
    expect(ids({ has: ['comments'] })).toEqual(['c']);
    expect(ids({ has: ['description'] })).toEqual(['b']);
    // Facets combine with AND.
    expect(ids({ labels: ['api'], q: 'docs' })).toEqual(['b']);
  });

  it('applies a saved filter (its open tasks)', () => {
    expect(ids({ saved: 'F' })).toEqual(['a', 'c']);
    expect(ids({ saved: 'F', priorities: [2] })).toEqual(['c']);
    // A saved filter that's gone matches nothing rather than everything.
    expect(ids({ saved: 'gone' })).toEqual([]);
  });

  it('becomes a saved-filter query', () => {
    expect(filterToQuery(EMPTY_FILTER, snap)).toBeNull();
    const f: TaskFilter = { ...EMPTY_FILTER, q: 'refund (v2)', labels: ['bug', 'api'], priorities: [2, 1], due: ['overdue', 'week'], statuses: ['wip'], lists: ['L'], has: ['links'], saved: 'F' };
    expect(filterToQuery(f, snap)).toBe('search: refund & search: \\(v2\\) & (@bug | @api) & (p1 | p2) & (overdue | 7 days) & status: In progress & #Work \\& play & has: links & (p1 | overdue)');
    expect(filterToQuery({ ...EMPTY_FILTER, labels: ['bug', 'api'], labelMode: 'all' }, snap)).toBe('@bug & @api');
    // Groups can't be nested: left out.
    expect(filterToQuery({ ...EMPTY_FILTER, saved: 'G', due: ['none'] }, snap)).toBe('no date');
    expect(describeFilter(f, snap)).toEqual(['“refund (v2)”', '@bug or @api', 'p1 or p2', 'overdue or next 7 days', 'In progress', '#Work & play', 'with links', 'Urgent']);
  });

  it('reads stored filters defensively', () => {
    expect(normalizeFilter(null)).toEqual(EMPTY_FILTER);
    expect(normalizeFilter({ q: 'x', labels: 'nope' as unknown as string[], priorities: [1, 9 as 1], labelMode: 'all' })).toEqual({ ...EMPTY_FILTER, q: 'x', priorities: [1], labelMode: 'all' });
  });
});

describe('what views hide', () => {
  // All tasks: open and completed tasks, a collapsed parent, completed tasks older than a week left out of the snapshot.
  const recent = new Date(2026, 9, 22, 12).getTime();
  const all = [
    task({ id: 'p', content: 'Parent', collapsed: true }),
    task({ id: 'k1', parentId: 'p' }),
    task({ id: 'k2', parentId: 'k1' }),
    task({ id: 'o', listId: 'P' }),
    task({ id: 'done1', done: true, completedAt: recent }),
    task({ id: 'done2', listId: 'inbox', done: true, completedAt: recent }),
  ];
  const s = snapshot(all, { olderDone: { L: 3, inbox: 1 } });

  it('counts the completed tasks of the view, loaded or not', () => {
    expect(olderDoneIn(s, { kind: 'all' })).toBe(4);
    expect(olderDoneIn(s, { kind: 'list', id: 'L' })).toBe(3);
    expect(olderDoneIn(s, { kind: 'inbox' })).toBe(1);
    expect(olderDoneIn(s, { kind: 'today' })).toBe(0);
  });

  it('All tasks shows every task, and says what it hides', () => {
    // Completed hidden (2 loaded + 4 older), sub-tasks folded under the collapsed parent.
    const st = listStats(s.tasks, false, undefined, olderDoneIn(s, { kind: 'all' }));
    expect(st).toMatchObject({ shown: 4, total: 4, unit: 'tasks' });
    expect(hiddenNotices(st.hidden)).toEqual([
      { kind: 'done', count: 6 },
      { kind: 'folded', count: 2, parents: ['p'] },
    ]);
    expect(st.hidden.map(hiddenText)).toEqual(['6 completed tasks hidden', '2 sub-tasks folded under collapsed tasks']);
    // The rows: the collapsed parent's sub-tasks stay folded; every list with tasks is there.
    const rows = allLayout(s, false);
    expect(rows.map((g) => [g.list.id, g.rows.map((r) => r.task.id)])).toEqual([
      ['P', ['o']],
      ['L', ['p']],
    ]);
    // Completed shown: every loaded one is a row; the older ones are a "Load".
    const shown = listStats(s.tasks, true, undefined, 4);
    expect(shown.total).toBe(6);
    expect(hiddenNotices(shown.hidden)).toEqual([
      { kind: 'older', count: 4 },
      { kind: 'folded', count: 2, parents: ['p'] },
    ]);
    expect(allLayout(s, true).flatMap((g) => g.rows.map((r) => r.task.id))).toEqual(['done2', 'o', 'p', 'done1']);
    // Everything loaded: nothing left to say.
    expect(hiddenNotices(listStats(s.tasks, true, undefined, 0).hidden).map((h) => h.kind)).toEqual(['folded']);
  });

  it('a filter opens collapsed parents and counts matches', () => {
    const match = (t: TaskView) => t.id === 'k2';
    const st = listStats(s.tasks, false, match, 0);
    expect(st).toMatchObject({ shown: 1, total: 4 });
    expect(st.hidden.some((h) => h.kind === 'folded')).toBe(false);
    const rows = allLayout(s, false, 'manual', match).flatMap((g) => g.rows);
    expect(rows.map((r) => [r.task.id, !!r.context])).toEqual([
      ['p', true],
      ['k1', true],
      ['k2', false],
    ]);
  });

  it('a status board shows sub-tasks as cards in All tasks and every completion on demand', () => {
    const board = snapshot([...all, task({ id: 'old', done: true, completedAt: new Date(2026, 9, 1).getTime() })]);
    const cards = (subtasks: 'cards' | 'nested', doneDays: number) =>
      boardColumns(board, boardTasks(board, { kind: 'all' }, 'status', false, doneDays), 'status', { sort: 'manual', today: TODAY, subtasks }).map((c) => [c.key, c.tasks.map((t) => t.id)]);
    expect(cards('cards', 0)).toEqual([
      ['status:todo', ['p', 'k1', 'k2', 'o']],
      ['status:wip', []],
      ['status:done', ['done1', 'done2', 'old']],
    ]);
    // Nested: sub-tasks inside the parent's card; the done window leaves the old completion out.
    expect(cards('nested', 7)).toEqual([
      ['status:todo', ['p', 'o']],
      ['status:wip', []],
      ['status:done', ['done1', 'done2']],
    ]);
  });

  it('orders notices and drops empty ones', () => {
    expect(hiddenNotices([{ kind: 'nested', count: 2 }, { kind: 'done', count: 0 }, { kind: 'window', count: 1, days: 7 }]).map((h) => h.kind)).toEqual(['window', 'nested']);
    expect(hiddenText({ kind: 'window', count: 1, days: 7 })).toBe('1 completed before the last 7 days not in the done column');
    expect(hiddenText({ kind: 'nested', count: 1 })).toBe('1 sub-task shown inside its parent card');
    expect(hiddenText({ kind: 'older', count: 2 })).toBe('2 tasks completed more than a week ago not loaded');
  });

  it('reads board preferences defensively', () => {
    expect(storedBoard(null)).toEqual({ doneDays: 7, doneCollapsed: false, collapsed: [], labelsExpanded: false });
    expect(storedBoard('{"doneDays":0,"collapsed":["p4",3],"labelsExpanded":true}')).toEqual({ doneDays: 0, doneCollapsed: false, collapsed: ['p4'], labelsExpanded: true });
    expect(storedBoard('{"doneDays":5}').doneDays).toBe(7);
    expect(storedBoard('not json').doneDays).toBe(7);
  });
});

describe('matchesFilter context', () => {
  it('uses the snapshot for sub-tasks and saved filters', () => {
    const cx = filterCtx(snap, { ...EMPTY_FILTER, saved: 'F' });
    expect([...cx.parents]).toEqual(['a']);
    expect(cx.saved && [...cx.saved]).toEqual(['a', 'c']);
    expect(matchesFilter(tasks[0], { ...EMPTY_FILTER, saved: 'F' }, cx)).toBe(true);
  });
});
