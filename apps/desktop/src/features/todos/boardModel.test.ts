import { describe, expect, it } from 'vitest';
import type { Snapshot, TaskView } from '@/lib/api/todos';
import { boardColumns, boardDrop, childCounts, columnDefaults, groupChoices, topLevel, viewTasks, type BoardColumn } from './boardModel';
import { resolveOptions } from './ui';

const TODAY = '2026-10-23'; // a Friday

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
    settings: { reminders: true, remindBeforeMinutes: 0 },
    tasks,
    projectList: 'P',
    completedCount: 0,
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

  it('offers groupings per view', () => {
    expect(groupChoices({ kind: 'list', id: 'L' })[0]).toBe('section');
    expect(groupChoices({ kind: 'all' })[0]).toBe('list');
    expect(groupChoices({ kind: 'upcoming' })[0]).toBe('due');
    expect(groupChoices({ kind: 'today' })[0]).toBe('priority');
    expect(resolveOptions({ kind: 'list', id: 'L' })).toEqual({ layout: 'list', groupBy: 'section', sort: 'manual' });
    expect(resolveOptions({ kind: 'today' }, { layout: 'board', groupBy: 'section' })).toEqual({ layout: 'board', groupBy: 'priority', sort: 'manual' });
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
    expect(cards(byDue, 'due:')).toEqual(['d']);
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

  it('adds tasks with the column as defaults', () => {
    expect(columnDefaults(now, 'inbox')).toEqual({ listId: 'L', sectionId: 's1' });
    expect(columnDefaults(none, 'inbox')).toEqual({ listId: 'L', sectionId: undefined });
    const byP = boardColumns(snap, [], 'priority', { sort: 'manual', today: TODAY });
    expect(columnDefaults(byP[1], 'P')).toEqual({ priority: 2, listId: 'P' });
    const byDue = boardColumns(snap, [], 'due', { sort: 'manual', today: TODAY });
    expect(columnDefaults(byDue[0], 'P')).toEqual({ due: { date: TODAY }, listId: 'P' });
    expect(columnDefaults(byDue[byDue.length - 1], 'P')).toEqual({ listId: 'P' });
  });
});
