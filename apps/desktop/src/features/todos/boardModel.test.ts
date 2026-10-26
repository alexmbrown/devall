import { describe, expect, it } from 'vitest';
import type { Snapshot, StatusColumn, TaskView } from '@/lib/api/todos';
import {
  boardColumns,
  boardDrop,
  boardTasks,
  cardChecklist,
  childCounts,
  columnCounts,
  columnDefaults,
  columnOrder,
  columnsError,
  columnKeyOf,
  foldSubtasks,
  groupChoices,
  inDoneWindow,
  insertColumn,
  moveColumn,
  statusChip,
  statusOf,
  topLevel,
  viewDefaults,
  viewTasks,
  type BoardColumn,
} from './boardModel';
import { resolveOptions } from './ui';

const TODAY = '2026-10-23'; // a Friday
/** Local noon `days` after TODAY, as Unix ms (completion times). */
const at = (days = 0) => new Date(2026, 9, 23 + days, 12).getTime();
const COLUMNS: StatusColumn[] = [
  { id: 'todo', name: 'To do' },
  { id: 'in-progress', name: 'In progress', color: 'blue' },
  { id: 'done', name: 'Done', color: 'green', done: true },
];

let n = 0;
function task(p: Partial<TaskView> & { id?: string }): TaskView {
  n++;
  return { id: p.id ?? `t${n}`, content: p.content ?? `Task ${n}`, listId: 'L', order: n, priority: 4, createdAt: 0, updatedAt: 0, ...p };
}

function snapshot(tasks: TaskView[]): Snapshot {
  return {
    lists: [
      { id: 'inbox', name: 'Inbox', kind: 'inbox', order: -1 },
      { id: 'L', name: 'Work', kind: 'user', order: 1 },
      { id: 'P', name: 'Shop', kind: 'project', order: 2 },
      { id: 'Q', name: 'Other project', kind: 'project', order: 3 },
    ],
    sections: [
      { id: 's2', listId: 'L', name: 'Later', order: 1 },
      { id: 's1', listId: 'L', name: 'Now', order: 0 },
    ],
    labels: [],
    filters: [{ id: 'F', name: 'Mine', query: 'p1', order: 0, count: 1, groups: [{ title: 'p1', ids: ['a'] }] }],
    settings: { reminders: true, remindBeforeMinutes: 0, columns: COLUMNS },
    tasks,
    projectList: 'P',
    completedCount: 0,
    olderDone: {},
    today: TODAY,
  };
}

const keys = (cols: BoardColumn[]) => cols.map((c) => c.key);
const cards = (cols: BoardColumn[], key: string) => cols.find((c) => c.key === key)!.tasks.map((t) => t.id);

describe('board columns', () => {
  const tasks = [
    task({ id: 'a', order: 1, priority: 1, due: { date: TODAY } }),
    task({ id: 'a1', parentId: 'a', order: 0 }),
    task({ id: 'b', order: 0, sectionId: 's1', due: { date: '2026-10-25' } }),
    task({ id: 'c', order: 0, sectionId: 's2', priority: 2, due: { date: '2026-10-20' } }),
    task({ id: 'd', order: 2, sectionId: 'gone' }),
    task({ id: 'i', listId: 'inbox', due: { date: '2026-12-01' } }),
    task({ id: 'x', order: 3, done: true }),
  ];
  const snap = snapshot(tasks);

  it('offers groupings per view: status columns first everywhere', () => {
    for (const v of [{ kind: 'list', id: 'L' }, { kind: 'inbox' }, { kind: 'all' }, { kind: 'today' }, { kind: 'upcoming' }, { kind: 'label', name: 'x' }, { kind: 'filter', id: 'F' }] as const) expect(groupChoices(v)[0]).toBe('status');
    expect(groupChoices({ kind: 'list', id: 'L' })).toEqual(['status', 'section', 'priority', 'due']);
    expect(groupChoices({ kind: 'all' })).toEqual(['status', 'list', 'priority', 'due']);
    expect(groupChoices({ kind: 'upcoming' })[1]).toBe('due');
    expect(resolveOptions({ kind: 'list', id: 'L' })).toEqual({ layout: 'list', groupBy: 'status', sort: 'manual', subtasks: 'nested', accent: null });
    // All tasks gives every task a card (sub-tasks too) unless told otherwise.
    expect(resolveOptions({ kind: 'all' }, { layout: 'board', groupBy: 'list' })).toEqual({ layout: 'board', groupBy: 'list', sort: 'manual', subtasks: 'cards', accent: null });
    expect(resolveOptions({ kind: 'all' }, { subtasks: 'nested', accent: 'blue' })).toMatchObject({ subtasks: 'nested', accent: 'blue' });
    expect(resolveOptions({ kind: 'today' }, { layout: 'board', groupBy: 'section' })).toEqual({ layout: 'board', groupBy: 'status', sort: 'manual', subtasks: 'nested', accent: null });
    expect(resolveOptions({ kind: 'completed' }, { layout: 'board' }).layout).toBe('list');
  });

  it('groups a list by section (sub-tasks fold into their parent)', () => {
    const cols = boardColumns(snap, viewTasks(snap, { kind: 'list', id: 'L' }, false), 'section', { listId: 'L', sort: 'manual', today: TODAY });
    expect(keys(cols)).toEqual(['section:', 'section:s1', 'section:s2']);
    expect(cards(cols, 'section:')).toEqual(['a', 'd']);
    expect(cards(cols, 'section:s1')).toEqual(['b']);
    expect(cols.every((c) => c.ordered)).toBe(true);
    expect(cols[1].section?.name).toBe('Now');
    // Completed cards only with "show completed", last.
    const withDone = boardColumns(snap, viewTasks(snap, { kind: 'list', id: 'L' }, true), 'section', { listId: 'L', sort: 'manual', today: TODAY });
    expect(cards(withDone, 'section:')).toEqual(['a', 'd', 'x']);
    // Sorted by an attribute: no manual positions.
    expect(boardColumns(snap, viewTasks(snap, { kind: 'list', id: 'L' }, false), 'section', { listId: 'L', sort: 'due', today: TODAY })[0].ordered).toBe(false);
  });

  it('groups All tasks by list', () => {
    const cols = boardColumns(snap, viewTasks(snap, { kind: 'all' }, false), 'list', { sort: 'manual', today: TODAY });
    // The open project's list stays even when empty; other projects only with tasks.
    expect(keys(cols)).toEqual(['list:inbox', 'list:P', 'list:L']);
    // No section first, then by section order.
    expect(cards(cols, 'list:L')).toEqual(['a', 'd', 'b', 'c']);
    expect(cols[2].target).toEqual({ kind: 'slot', listId: 'L', sectionId: null, wholeList: true });
  });

  it('groups by priority and by due day', () => {
    const open = viewTasks(snap, { kind: 'all' }, false);
    const byP = boardColumns(snap, open, 'priority', { sort: 'manual', today: TODAY });
    expect(keys(byP)).toEqual(['p1', 'p2', 'p3', 'p4']);
    expect(cards(byP, 'p1')).toEqual(['a']);
    expect(byP.some((c) => c.ordered)).toBe(false);
    const byDue = boardColumns(snap, open, 'due', { sort: 'manual', today: TODAY });
    expect(keys(byDue)).toEqual(['due:overdue', 'due:2026-10-23', 'due:2026-10-24', 'due:2026-10-25', 'due:2026-10-26', 'due:2026-10-27', 'due:2026-10-28', 'due:2026-10-29', 'due:later', 'due:']);
    expect(cards(byDue, 'due:overdue')).toEqual(['c']);
    expect(cards(byDue, 'due:2026-10-25')).toEqual(['b']);
    expect(cards(byDue, 'due:later')).toEqual(['i']);
    // a1 has no date while its parent is due today: a card of its own (folded into a card in
    // another column, it would be on the board nowhere).
    expect(cards(byDue, 'due:')).toEqual(['a1', 'd']);
    expect(byDue[0]).toMatchObject({ target: { kind: 'none' }, tone: 'overdue' });
  });

  it('shows what each view shows', () => {
    expect(viewTasks(snap, { kind: 'today' }, true).map((t) => t.id)).toEqual(['a', 'c']);
    expect(viewTasks(snap, { kind: 'upcoming' }, false).map((t) => t.id)).toEqual(['a', 'b', 'c', 'i']);
    expect(viewTasks(snap, { kind: 'filter', id: 'F' }, false).map((t) => t.id)).toEqual(['a']);
    expect(viewTasks(snap, { kind: 'inbox' }, false).map((t) => t.id)).toEqual(['i']);
    expect(topLevel([tasks[0], tasks[1]]).map((t) => t.id)).toEqual(['a']);
    expect(topLevel([tasks[1]]).map((t) => t.id)).toEqual(['a1']);
    expect(childCounts(tasks).get('a')).toEqual({ open: 1, all: 1 });
  });
});

describe('board drops', () => {
  const tasks = [
    task({ id: 'a', order: 0 }),
    task({ id: 'b', order: 1 }),
    task({ id: 'x', order: 2, done: true }),
    task({ id: 'c', order: 3 }),
    task({ id: 's', order: 0, sectionId: 's1', priority: 2, due: { date: TODAY } }),
  ];
  const snap = snapshot(tasks);
  const cols = boardColumns(snap, viewTasks(snap, { kind: 'list', id: 'L' }, false), 'section', { listId: 'L', sort: 'manual', today: TODAY });
  const none = cols[0];
  const now = cols[1];

  it('reorders within an ordered column (positions among open cards)', () => {
    expect(boardDrop(tasks, none, ['a'], { id: 'c', pos: 'after' })).toEqual({ kind: 'move', target: { listId: 'L', sectionId: null, parentId: null, index: 2 } });
    expect(boardDrop(tasks, none, ['c'], { id: 'a', pos: 'before' })).toEqual({ kind: 'move', target: { listId: 'L', sectionId: null, parentId: null, index: 0 } });
    expect(boardDrop(tasks, none, ['a'], { id: 'b', pos: 'after' })).toEqual({ kind: 'move', target: { listId: 'L', sectionId: null, parentId: null, index: 1 } });
  });

  it('moves between section and list columns', () => {
    expect(boardDrop(tasks, now, ['a'])).toEqual({ kind: 'move', target: { listId: 'L', sectionId: 's1', parentId: null, index: null } });
    expect(boardDrop(tasks, now, ['a'], { id: 's', pos: 'before' })).toEqual({ kind: 'move', target: { listId: 'L', sectionId: 's1', parentId: null, index: 0 } });
    const byList = boardColumns(snap, viewTasks(snap, { kind: 'all' }, false), 'list', { sort: 'manual', today: TODAY });
    const inbox = byList.find((c) => c.key === 'list:inbox')!;
    const work = byList.find((c) => c.key === 'list:L')!;
    expect(boardDrop(tasks, inbox, ['s'])).toEqual({ kind: 'move', target: { listId: 'inbox', sectionId: null, parentId: null, index: null } });
    // Dropped back into its own list column's empty space: nothing changes (keeps its section).
    expect(boardDrop(tasks, work, ['s'])).toBeNull();
    // Dropped on a card of another section in a list column: lands in that card's section.
    expect(boardDrop(tasks, work, ['a'], { id: 's', pos: 'after' })).toEqual({ kind: 'move', target: { listId: 'L', sectionId: 's1', parentId: null, index: 1 } });
  });

  it('updates priority or date in attribute columns', () => {
    const byP = boardColumns(snap, viewTasks(snap, { kind: 'all' }, false), 'priority', { sort: 'manual', today: TODAY });
    expect(boardDrop(tasks, byP[0], ['a', 's'])).toEqual({ kind: 'update', patch: { priority: 1 } });
    expect(boardDrop(tasks, byP[1], ['s'])).toBeNull();
    // Card positions don't matter there.
    expect(boardDrop(tasks, byP[0], ['a'], { id: 'b', pos: 'before' })).toEqual({ kind: 'update', patch: { priority: 1 } });
    const byDue = boardColumns(snap, viewTasks(snap, { kind: 'all' }, false), 'due', { sort: 'manual', today: TODAY });
    const tomorrow = byDue.find((c) => c.key === 'due:2026-10-24')!;
    expect(boardDrop(tasks, tomorrow, ['s'])).toEqual({ kind: 'update', patch: { dueDate: '2026-10-24' } });
    expect(boardDrop(tasks, byDue.find((c) => c.key === 'due:')!, ['s'])).toEqual({ kind: 'update', patch: { dueDate: '' } });
    expect(boardDrop(tasks, byDue.find((c) => c.key === `due:${TODAY}`)!, ['s'])).toBeNull();
    expect(boardDrop(tasks, { ...tomorrow, target: { kind: 'none' } }, ['s'])).toBeNull();
    expect(boardDrop(tasks, tomorrow, ['gone'])).toBeNull();
  });

  it('adds tasks with the view and the column as defaults', () => {
    expect(columnDefaults(now, { listId: 'inbox' })).toEqual({ listId: 'L', sectionId: 's1' });
    expect(columnDefaults(none, { listId: 'inbox' })).toEqual({ listId: 'L', sectionId: undefined });
    const byP = boardColumns(snap, [], 'priority', { sort: 'manual', today: TODAY });
    expect(columnDefaults(byP[1], { listId: 'P' })).toEqual({ priority: 2, listId: 'P' });
    const byDue = boardColumns(snap, [], 'due', { sort: 'manual', today: TODAY });
    expect(columnDefaults(byDue[0], { listId: 'P' })).toEqual({ due: { date: TODAY }, listId: 'P' });
    // "No date" drops the view's date (Today).
    expect(columnDefaults(byDue[byDue.length - 1], viewDefaults({ kind: 'today' }, snap))).toEqual({ listId: 'P' });
    // Status columns keep the view's defaults (Today's date, the label) and set the status; the done column adds none.
    const byStatus = boardColumns(snap, [], 'status', { sort: 'manual', today: TODAY });
    expect(columnDefaults(byStatus[1], viewDefaults({ kind: 'today' }, snap))).toEqual({ due: { date: TODAY }, listId: 'P', status: 'in-progress' });
    expect(columnDefaults(byStatus[0], viewDefaults({ kind: 'label', name: 'bug' }, snap))).toEqual({ labels: ['bug'], listId: 'P', status: 'todo' });
    expect(columnDefaults(byStatus[2], viewDefaults({ kind: 'list', id: 'L' }, snap))).toEqual({ listId: 'L' });
  });
});

describe('status columns', () => {
  const tasks = [
    task({ id: 'a', order: 1 }),
    task({ id: 'b', order: 0, status: 'in-progress', priority: 1 }),
    task({ id: 'c', order: 2, status: 'gone' }),
    task({ id: 'd', order: 3, status: 'in-progress', statusOrder: 0, sectionId: 's1' }),
    task({ id: 'i', listId: 'inbox', order: 0, status: 'in-progress', due: { date: TODAY } }),
    task({ id: 'x', order: 4, done: true, completedAt: at(0), status: 'in-progress' }),
    task({ id: 'y', order: 5, done: true, completedAt: at(-1) }),
    task({ id: 'old', order: 6, done: true, completedAt: at(-8) }),
    task({ id: 'p', listId: 'P', order: 0, status: 'done' }),
  ];
  const snap = snapshot(tasks);
  const cols = (view: Parameters<typeof viewTasks>[1], doneDays = 7, sort: 'manual' | 'priority' = 'manual') =>
    boardColumns(snap, boardTasks(snap, view, 'status', false, doneDays), 'status', { listId: view.kind === 'list' ? view.id : undefined, sort, today: TODAY });

  it('resolves a task’s column like Rust does', () => {
    expect(statusOf(tasks[0], COLUMNS)?.id).toBe('todo');
    expect(statusOf(tasks[1], COLUMNS)?.id).toBe('in-progress');
    expect(statusOf(tasks[2], COLUMNS)?.id).toBe('todo'); // unknown column → the first
    expect(statusOf(tasks[5], COLUMNS)?.id).toBe('done'); // completed: the done column, whatever its status
    expect(statusOf(tasks[8], COLUMNS)?.id).toBe('todo'); // open but pointing at the done column → the first
    // The first column is the first that isn't the done column.
    expect(statusOf(tasks[0], [COLUMNS[2], COLUMNS[1], COLUMNS[0]])?.id).toBe('in-progress');
    // Chips only for columns between the first and the done one.
    expect(statusChip(tasks[1], COLUMNS)?.name).toBe('In progress');
    expect([tasks[0], tasks[5]].map((t) => statusChip(t, COLUMNS))).toEqual([undefined, undefined]);
    expect(Object.fromEntries(columnCounts(tasks, COLUMNS))).toEqual({ todo: 3, 'in-progress': 3 });
  });

  it('groups by status, with the recent completions in the done column', () => {
    const list = cols({ kind: 'list', id: 'L' });
    expect(list.map((c) => c.key)).toEqual(['status:todo', 'status:in-progress', 'status:done']);
    expect(cards(list, 'status:todo')).toEqual(['a', 'c']);
    // Manual order: position in the column, then list, section, position.
    expect(cards(list, 'status:in-progress')).toEqual(['b', 'd']);
    // Newest completion first; older than the done window stays out (whatever "show completed" says).
    expect(cards(list, 'status:done')).toEqual(['x', 'y']);
    expect(cards(cols({ kind: 'list', id: 'L' }, 1), 'status:done')).toEqual(['x']);
    expect(list.map((c) => c.ordered)).toEqual([true, true, false]);
    expect(list[2].target).toEqual({ kind: 'status', status: 'done', done: true });
    expect(list[1].column?.color).toBe('blue');
    // Sorted by an attribute: no manual positions.
    const byP = cols({ kind: 'list', id: 'L' }, 7, 'priority');
    expect(byP[1].ordered).toBe(false);
    // Across lists: Inbox first.
    expect(cards(cols({ kind: 'all' }), 'status:in-progress')).toEqual(['i', 'b', 'd']);
    // Today: open tasks due today or earlier; done ones due by today.
    expect(cards(cols({ kind: 'today' }), 'status:in-progress')).toEqual(['i']);
    expect(cards(cols({ kind: 'today' }), 'status:done')).toEqual([]);
    // Filters only know open tasks.
    expect(boardTasks(snap, { kind: 'filter', id: 'F' }, 'status', false, 7).every((t) => !t.done)).toBe(true);
    // Other groupings keep the "show completed" rule.
    expect(boardTasks(snap, { kind: 'list', id: 'L' }, 'section', false, 7).some((t) => t.done)).toBe(false);
  });

  it('gives a sub-task in another column than its parent a card of its own', () => {
    // Positions in the column as Rust assigns them (each new task at the end of its column).
    const parent = task({ id: 'P1', order: 10, statusOrder: 1 });
    const same = task({ id: 'S1', parentId: 'P1', order: 0, statusOrder: 2 });
    const moved = task({ id: 'S2', parentId: 'P1', order: 1, status: 'in-progress' });
    const s = snapshot([parent, same, moved]);
    const keyOf = columnKeyOf(s, 'status', { today: TODAY });
    expect(foldSubtasks([parent, same, moved], keyOf, 'nested')).toEqual({ cards: [parent, moved], folded: [same] });
    expect(foldSubtasks([parent, same, moved], keyOf, 'cards')).toEqual({ cards: [parent, same, moved], folded: [] });
    const board = boardColumns(s, [parent, same, moved], 'status', { sort: 'manual', today: TODAY });
    expect(cards(board, 'status:todo')).toEqual(['P1']);
    expect(cards(board, 'status:in-progress')).toEqual(['S2']);
    expect(cards(boardColumns(s, [parent, same, moved], 'status', { sort: 'manual', today: TODAY, subtasks: 'cards' }), 'status:todo')).toEqual(['P1', 'S1']);
    // Column keys per grouping.
    expect(columnKeyOf(s, 'section', { listId: 'L', today: TODAY })(task({ sectionId: 'gone' }))).toBe('section:');
    expect(columnKeyOf(s, 'section', { listId: 'L', today: TODAY })(task({ listId: 'P' }))).toBeNull();
    expect(columnKeyOf(s, 'due', { today: TODAY })(task({ due: { date: '2026-11-30' } }))).toBe('due:later');
    expect(columnKeyOf(s, 'priority', { today: TODAY })(task({ priority: 2 }))).toBe('p2');
  });

  it('counts a card’s checklist: sub-tasks and description task lists', () => {
    expect(cardChecklist({ description: '- [x] a\n- [ ] b' }, { open: 1, all: 3 })).toEqual({ done: 3, total: 5 });
    expect(cardChecklist({}, undefined)).toEqual({ done: 0, total: 0 });
  });

  it('keeps completions in the done column’s window (0: all)', () => {
    expect(inDoneWindow({ completedAt: at(0) }, TODAY, 1)).toBe(true);
    expect(inDoneWindow({ completedAt: at(-1) }, TODAY, 1)).toBe(false);
    expect(inDoneWindow({ completedAt: at(-6) }, TODAY, 7)).toBe(true);
    expect(inDoneWindow({ completedAt: at(-8) }, TODAY, 7)).toBe(false);
    expect(inDoneWindow({ completedAt: at(-400) }, TODAY, 0)).toBe(true);
    expect(cards(cols({ kind: 'list', id: 'L' }, 0), 'status:done')).toEqual(['x', 'y', 'old']);
  });

  it('orders a column by its manual position first', () => {
    const order = columnOrder(snap);
    const ids = (list: TaskView[]) => [...list].sort(order).map((t) => t.id);
    expect(ids([task({ id: 'm', statusOrder: 2 }), task({ id: 'n', statusOrder: 1, listId: 'P' }), task({ id: 'o', statusOrder: 1, listId: 'inbox' })])).toEqual(['o', 'n', 'm']);
  });

  it('drops between status columns', () => {
    const list = cols({ kind: 'list', id: 'L' });
    const [todo, wip, done] = list;
    expect(boardDrop(tasks, wip, ['a'])).toEqual({ kind: 'status', status: 'in-progress' });
    expect(boardDrop(tasks, wip, ['a'], { id: 'd', pos: 'before' })).toEqual({ kind: 'status', status: 'in-progress', before: 'd' });
    expect(boardDrop(tasks, wip, ['a'], { id: 'b', pos: 'after' })).toEqual({ kind: 'status', status: 'in-progress', after: 'b' });
    // Reordering inside a column; dropping on its empty space changes nothing.
    expect(boardDrop(tasks, todo, ['c'], { id: 'a', pos: 'before' })).toEqual({ kind: 'status', status: 'todo', before: 'a' });
    expect(boardDrop(tasks, todo, ['c'])).toBeNull();
    // Into the done column: completes (positions don't matter); already done: nothing.
    expect(boardDrop(tasks, done, ['a', 'b'], { id: 'x', pos: 'before' })).toEqual({ kind: 'status', status: 'done' });
    expect(boardDrop(tasks, done, ['x'])).toBeNull();
    // Out of the done column: reopens there.
    expect(boardDrop(tasks, todo, ['x'])).toEqual({ kind: 'status', status: 'todo' });
    expect(boardDrop(tasks, wip, ['gone'])).toBeNull();
  });

  it('edits the column list', () => {
    expect(moveColumn(COLUMNS, 'in-progress', -1).map((c) => c.id)).toEqual(['in-progress', 'todo', 'done']);
    expect(moveColumn(COLUMNS, 'done', 1)).toBe(COLUMNS);
    expect(insertColumn(COLUMNS, ' Review ').map((c) => c.name)).toEqual(['To do', 'In progress', 'Review', 'Done']);
    expect(insertColumn(moveColumn(COLUMNS, 'done', -1), 'Later').map((c) => c.name)).toEqual(['To do', 'Done', 'In progress', 'Later']);
    expect(columnsError(COLUMNS)).toBeNull();
    expect(columnsError([...COLUMNS, { id: '', name: 'to  DO' }])).toContain('To do');
    expect(columnsError([COLUMNS[0], { ...COLUMNS[1], done: true }, COLUMNS[2]])).toContain('exactly one');
    expect(columnsError([COLUMNS[2]])).toContain('besides');
    expect(columnsError([{ id: '', name: ' ' }, COLUMNS[2]])).toContain('name');
  });
});
