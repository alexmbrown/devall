import { describe, expect, it } from 'vitest';
import type { QuickToken, Snapshot, TaskView } from '@/lib/api/todos';
import {
  addDays,
  allLayout,
  appDocument,
  attention,
  boardKeyAction,
  boardNav,
  byDue,
  classifyLink,
  colorVar,
  compareBy,
  defaultLabelColor,
  foldedUnder,
  halfOf,
  isAbsolutePath,
  joinPath,
  navOrder,
  reorderIds,
  dayHeading,
  dayLabel,
  daysBetween,
  dropPosition,
  dropTarget,
  dueInfo,
  indentTarget,
  keyAction,
  labelColor,
  linkLabel,
  listLayout,
  markdownChecklist,
  monthGrid,
  moveCursor,
  outdentTarget,
  parseViewKey,
  rangeSelect,
  ruleText,
  searchTasks,
  segments,
  sigilAt,
  startOfWeek,
  subtree,
  suggest,
  todayGroups,
  toneOf,
  treeRows,
  upcomingDays,
  viewKey,
  weekDays,
} from './lib';

const TODAY = '2026-10-23'; // a Friday

let n = 0;
function task(p: Partial<TaskView> & { id?: string }): TaskView {
  n++;
  return { id: p.id ?? `t${n}`, content: p.content ?? `Task ${n}`, listId: 'inbox', order: n, priority: 4, createdAt: 0, updatedAt: 0, ...p };
}

describe('dates', () => {
  it('does calendar arithmetic on local days', () => {
    expect(addDays('2026-10-31', 1)).toBe('2026-11-01');
    expect(addDays('2026-12-31', 1)).toBe('2027-01-01');
    expect(addDays('2028-02-28', 1)).toBe('2028-02-29');
    expect(addDays('2026-03-01', -1)).toBe('2026-02-28');
    // Across DST changes (Europe and US) a day is still a day.
    expect(addDays('2026-03-28', 2)).toBe('2026-03-30');
    expect(addDays('2026-11-01', 1)).toBe('2026-11-02');
    expect(daysBetween('2026-03-07', '2026-03-09')).toBe(2);
    expect(daysBetween('2026-10-24', '2026-10-26')).toBe(2);
    expect(daysBetween(TODAY, '2026-10-20')).toBe(-3);
  });

  it('weeks start on Monday', () => {
    expect(startOfWeek(TODAY)).toBe('2026-10-19');
    expect(startOfWeek('2026-10-25')).toBe('2026-10-19');
    expect(startOfWeek('2026-10-19')).toBe('2026-10-19');
    expect(weekDays('2026-10-19')).toEqual(['2026-10-19', '2026-10-20', '2026-10-21', '2026-10-22', '2026-10-23', '2026-10-24', '2026-10-25']);
    const grid = monthGrid('2026-10-15');
    expect(grid).toHaveLength(42);
    expect(grid[0]).toBe('2026-09-28');
    expect(grid).toContain('2026-10-31');
  });

  it('labels days relative to today', () => {
    expect(dayLabel(TODAY, TODAY)).toBe('Today');
    expect(dayLabel('2026-10-24', TODAY)).toBe('Tomorrow');
    expect(dayLabel('2026-10-22', TODAY)).toBe('Yesterday');
    expect(dayLabel('2026-10-26', TODAY, 'en-US')).toBe('Mon');
    expect(dayLabel('2026-11-30', TODAY, 'en-US')).toBe('Nov 30');
    expect(dayLabel('2027-01-15', TODAY, 'en-US')).toBe('Jan 15, 2027');
    expect(dayLabel('2026-10-01', TODAY, 'en-US')).toBe('Oct 1');
    expect(dayHeading(TODAY, TODAY, 'en-US')).toBe('Today · Fri, Oct 23');
    expect(dayHeading('2026-10-26', TODAY, 'en-US')).toBe('Mon, Oct 26');
    expect([toneOf('2026-10-20', TODAY), toneOf(TODAY, TODAY), toneOf('2026-10-24', TODAY), toneOf('2026-10-27', TODAY), toneOf('2026-11-27', TODAY)]).toEqual(['overdue', 'today', 'tomorrow', 'soon', 'later']);
  });

  it('describes due dates', () => {
    expect(dueInfo(task({}), TODAY)).toBeNull();
    const noon = new Date(2026, 9, 23, 12, 0).getTime();
    const late = dueInfo(task({ due: { date: TODAY, time: '09:00' }, dueAt: new Date(2026, 9, 23, 9, 0).getTime() }), TODAY, noon, 'en-GB');
    expect(late).toMatchObject({ text: 'Today', tone: 'today', time: '9:00', late: true, recurring: false });
    const r = dueInfo(task({ due: { date: '2026-10-26', recurrence: { freq: 'week' }, string: 'every mon' } }), TODAY);
    expect(r).toMatchObject({ tone: 'soon', recurring: true, late: false });
    // Fixed-zone tasks use the local day the server computed.
    expect(dueInfo(task({ due: { date: TODAY, time: '23:30', timezone: 'America/New_York' }, dueLocal: '2026-10-24' }), TODAY)?.text).toBe('Tomorrow');
    expect(ruleText({ date: TODAY, recurrence: { freq: 'week', interval: 2, weekdays: ['mon'] } })).toBe('every 2 weeks on mon');
    expect(ruleText({ date: TODAY, recurrence: { freq: 'day', fromCompletion: true } })).toBe('every! day');
    expect(ruleText({ date: TODAY, recurrence: { freq: 'month' }, string: 'every month on the 1st' })).toBe('every month on the 1st');
  });
});

describe('views', () => {
  const tasks = [
    task({ id: 'late', due: { date: '2026-10-20' }, priority: 2 }),
    task({ id: 'p1', due: { date: TODAY }, priority: 1 }),
    task({ id: 'timed', due: { date: TODAY, time: '08:00' }, dueAt: new Date(2026, 9, 23, 8).getTime() }),
    task({ id: 'plain', due: { date: TODAY } }),
    task({ id: 'next', due: { date: '2026-10-26' } }),
    task({ id: 'far', due: { date: '2026-12-01' } }),
    task({ id: 'none' }),
    task({ id: 'done', due: { date: TODAY }, done: true }),
  ];

  it('sorts by day, time, priority and position', () => {
    expect([...tasks].sort(byDue).map((t) => t.id)).toEqual(['late', 'timed', 'p1', 'plain', 'done', 'next', 'far', 'none']);
  });

  it('groups Today and Upcoming', () => {
    expect(attention(tasks, TODAY).map((t) => t.id)).toEqual(['late', 'p1', 'timed', 'plain']);
    const g = todayGroups(tasks, TODAY);
    expect(g.overdue.map((t) => t.id)).toEqual(['late']);
    expect(g.today.map((t) => t.id)).toEqual(['timed', 'p1', 'plain']);
    const u = upcomingDays(tasks, TODAY, 7);
    expect(u.overdue.map((t) => t.id)).toEqual(['late']);
    expect(u.days).toHaveLength(7);
    expect(u.days[0]).toMatchObject({ date: TODAY });
    expect(u.days[0].tasks.map((t) => t.id)).toEqual(['timed', 'p1', 'plain']);
    expect(u.days[3].tasks.map((t) => t.id)).toEqual(['next']);
    expect(u.days.flatMap((d) => d.tasks).some((t) => t.id === 'far')).toBe(false);
  });

  it('builds the task tree', () => {
    const t = [
      task({ id: 'a', order: 0 }),
      task({ id: 'a1', parentId: 'a', order: 0 }),
      task({ id: 'a2', parentId: 'a', order: 1, done: true }),
      task({ id: 'a1x', parentId: 'a1', order: 0 }),
      task({ id: 'b', order: 1, collapsed: true }),
      task({ id: 'b1', parentId: 'b', order: 0 }),
      task({ id: 'orphan', parentId: 'gone', order: 2 }),
      task({ id: 'd1', parentId: 'd0' }),
    ];
    const rows = treeRows(t, false);
    expect(rows.map((r) => `${r.depth}:${r.task.id}`)).toEqual(['0:a', '1:a1', '2:a1x', '0:b', '0:orphan', '0:d1']);
    expect(rows[0]).toMatchObject({ open: 1, kids: 2 });
    expect(rows[3]).toMatchObject({ open: 1, kids: 1 });
    expect(treeRows(t, true).map((r) => r.task.id)).toContain('a2');
    expect(treeRows(t, false, 1).map((r) => r.depth)).toEqual([0, 1, 1, 0, 0, 0]);
    expect(subtree(t, 'a')).toEqual(['a', 'a1', 'a2', 'a1x']);
    // What the collapsed task hides.
    expect(foldedUnder(t, false)).toEqual({ count: 1, parents: ['b'] });
    expect(foldedUnder(t, true)).toEqual({ count: 1, parents: ['b'] });
  });

  it('never hides an open sub-task behind a hidden completed parent', () => {
    const t = [task({ id: 'p', order: 0, done: true }), task({ id: 'k', parentId: 'p', order: 0 }), task({ id: 'q', order: 1 })];
    expect(treeRows(t, false).map((r) => `${r.depth}:${r.task.id}`)).toEqual(['0:k', '0:q']);
    expect(treeRows(t, true).map((r) => `${r.depth}:${r.task.id}`)).toEqual(['0:q', '0:p', '1:k']);
  });

  it('filters the tree: matches with their parents, collapsed ones opened', () => {
    const t = [task({ id: 'a', order: 0, collapsed: true }), task({ id: 'a1', parentId: 'a', order: 0, content: 'needle' }), task({ id: 'b', order: 1, content: 'needle too' }), task({ id: 'c', order: 2 })];
    const rows = treeRows(t, false, 4, 'manual', (x) => x.content.startsWith('needle'));
    expect(rows.map((r) => [r.task.id, !!r.context])).toEqual([
      ['a', true],
      ['a1', false],
      ['b', false],
    ]);
    // The parent's counts stay those of all its sub-tasks.
    expect(rows[0]).toMatchObject({ open: 1, kids: 1 });
  });

  it('lays out a list with sections', () => {
    const snap = {
      sections: [
        { id: 's2', listId: 'L', name: 'Later', order: 1 },
        { id: 's1', listId: 'L', name: 'Now', order: 0, collapsed: true },
        { id: 'x', listId: 'other', name: 'X', order: 0 },
      ],
      tasks: [task({ id: 'n', listId: 'L' }), task({ id: 'i', listId: 'L', sectionId: 's1' }), task({ id: 'j', listId: 'L', sectionId: 's2' }), task({ id: 'k', listId: 'L', sectionId: 'deleted' }), task({ id: 'o', listId: 'other' })],
    } as unknown as Snapshot;
    const l = listLayout(snap, 'L', false);
    expect(l.rows.map((r) => r.task.id)).toEqual(['n', 'k']);
    expect(l.sections.map((s) => [s.section.id, s.rows.map((r) => r.task.id), s.open])).toEqual([
      ['s1', ['i'], 1],
      ['s2', ['j'], 1],
    ]);
  });

  it('sorts list rows by an attribute', () => {
    const t = [
      task({ id: 'b', content: 'beta', order: 0, priority: 3, createdAt: 2 }),
      task({ id: 'a', content: 'Alpha 10', order: 1, priority: 1, createdAt: 1, due: { date: '2026-10-30' } }),
      task({ id: 'c', content: 'alpha 9', order: 2, priority: 1, createdAt: 3, due: { date: '2026-10-24' } }),
    ];
    const ids = (s: Parameters<typeof compareBy>[0]) => treeRows(t, false, 4, s).map((r) => r.task.id);
    expect(ids('manual')).toEqual(['b', 'a', 'c']);
    expect(ids('due')).toEqual(['c', 'a', 'b']);
    expect(ids('priority')).toEqual(['c', 'a', 'b']);
    expect(ids('added')).toEqual(['c', 'b', 'a']);
    expect(ids('name')).toEqual(['c', 'a', 'b']);
  });

  it('lays out All tasks by list in the left pane order', () => {
    const snap = {
      lists: [
        { id: 'u2', name: 'Side', kind: 'user', order: 5 },
        { id: 'P', name: 'Shop', kind: 'project', order: 9 },
        { id: 'inbox', name: 'Inbox', kind: 'inbox', order: -1 },
        { id: 'u1', name: 'Work', kind: 'user', order: 2 },
        { id: 'Q', name: 'Other', kind: 'project', order: 1 },
      ],
      projectList: 'P',
      sections: [{ id: 's', listId: 'u1', name: 'Later', order: 0 }],
      tasks: [task({ id: 'w1', listId: 'u1', sectionId: 's' }), task({ id: 'w0', listId: 'u1' }), task({ id: 'i', listId: 'inbox' }), task({ id: 'p', listId: 'P' }), task({ id: 'd', listId: 'u2', done: true })],
    } as unknown as Snapshot;
    expect(navOrder(snap.lists, 'P').map((l) => l.id)).toEqual(['inbox', 'P', 'u1', 'u2', 'Q']);
    const groups = allLayout(snap, false);
    expect(groups.map((g) => [g.list.id, g.rows.map((r) => r.task.id), g.open])).toEqual([
      ['inbox', ['i'], 1],
      ['P', ['p'], 1],
      ['u1', ['w0', 'w1'], 2],
    ]);
    expect(allLayout(snap, true).map((g) => g.list.id)).toContain('u2');
  });

  it('searches content, description, labels and comments', () => {
    const t = [task({ id: 'a', content: 'Refund flow', labels: ['api'] }), task({ id: 'b', content: 'Docs', description: 'mention refund limits' }), task({ id: 'c', content: 'x', comments: [{ id: 'c', text: 'Refund done?', at: 0 }] })];
    expect(searchTasks(t, 'refund').map((x) => x.id)).toEqual(['a', 'b', 'c']);
    expect(searchTasks(t, 'refund @api').map((x) => x.id)).toEqual(['a']);
    expect(searchTasks(t, '  ')).toEqual([]);
  });
});

describe('moving', () => {
  const t = [task({ id: 'a', order: 0 }), task({ id: 'b', order: 1 }), task({ id: 'c', order: 2 }), task({ id: 'b1', parentId: 'b', order: 0 }), task({ id: 'b2', parentId: 'b', order: 1 }), task({ id: 's', sectionId: 'S', order: 0 })];

  it('computes drop targets', () => {
    const rect = { top: 100, height: 40, left: 0 };
    expect(dropPosition(rect, 10, 105, 0)).toBe('before');
    expect(dropPosition(rect, 10, 130, 0)).toBe('after');
    expect(dropPosition(rect, 120, 130, 0)).toBe('child');
    expect(dropPosition(rect, 120, 130, 4)).toBe('after');
    const c = t.find((x) => x.id === 'c')!;
    expect(dropTarget(t, c, 'before', ['a'])).toEqual({ listId: 'inbox', sectionId: null, parentId: null, index: 1 });
    expect(dropTarget(t, c, 'after', ['a'])).toEqual({ listId: 'inbox', sectionId: null, parentId: null, index: 2 });
    expect(dropTarget(t, c, 'child', ['a'])).toEqual({ listId: 'inbox', sectionId: null, parentId: 'c', index: null });
    const b2 = t.find((x) => x.id === 'b2')!;
    expect(dropTarget(t, b2, 'before', ['a'])).toEqual({ listId: 'inbox', sectionId: null, parentId: 'b', index: 1 });
  });

  it('drop positions are among open siblings (what Rust counts)', () => {
    // Completed siblings stay in the data; the index must not count them.
    const ts = [task({ id: 'a', order: 0 }), task({ id: 'x', order: 1, done: true }), task({ id: 'b', order: 2 }), task({ id: 'c', order: 3 })];
    const b = ts.find((x) => x.id === 'b')!;
    expect(dropTarget(ts, b, 'after', ['a'])).toEqual({ listId: 'inbox', sectionId: null, parentId: null, index: 1 });
    expect(dropTarget(ts, b, 'before', ['c'])).toEqual({ listId: 'inbox', sectionId: null, parentId: null, index: 1 });
  });

  it('reorders ids before/after a target', () => {
    expect(reorderIds(['a', 'b', 'c', 'd'], 'a', 'c', 'after')).toEqual(['b', 'c', 'a', 'd']);
    expect(reorderIds(['a', 'b', 'c', 'd'], 'd', 'b', 'before')).toEqual(['a', 'd', 'b', 'c']);
    expect(reorderIds(['a', 'b', 'c'], 'c', 'a', 'before')).toEqual(['c', 'a', 'b']);
    expect(reorderIds(['a', 'b'], 'a', 'a', 'after')).toEqual(['a', 'b']);
    expect(reorderIds(['a', 'b'], 'z', 'a', 'after')).toEqual(['a', 'b']);
    expect(reorderIds(['a', 'b'], 'a', 'z', 'after')).toEqual(['a', 'b']);
    expect(halfOf({ top: 10, height: 20 }, 15)).toBe('before');
    expect(halfOf({ top: 10, height: 20 }, 25)).toBe('after');
  });

  it('indents and outdents', () => {
    const get = (id: string) => t.find((x) => x.id === id)!;
    expect(indentTarget(t, get('a'))).toBeNull();
    expect(indentTarget(t, get('c'))).toEqual({ listId: 'inbox', sectionId: null, parentId: 'b', index: null });
    expect(indentTarget(t, get('b2'))).toMatchObject({ parentId: 'b1' });
    expect(indentTarget(t, get('s'))).toBeNull();
    expect(outdentTarget(t, get('a'))).toBeNull();
    expect(outdentTarget(t, get('b1'))).toEqual({ listId: 'inbox', sectionId: null, parentId: null, index: 2 });
  });
});

describe('keyboard and selection', () => {
  it('maps keys', () => {
    expect(keyAction({ key: 'q' })).toBe('quickAdd');
    expect(keyAction({ key: '/' })).toBe('search');
    expect(keyAction({ key: 'j' })).toBe('down');
    expect(keyAction({ key: 'ArrowUp' })).toBe('up');
    expect(keyAction({ key: 'Enter' })).toBe('edit');
    expect(keyAction({ key: 'Enter', shiftKey: true })).toBe('open');
    expect(keyAction({ key: 'Enter', ctrlKey: true })).toBe('complete');
    expect(keyAction({ key: ' ' })).toBe('complete');
    expect(keyAction({ key: '3' })).toBe('p3');
    expect(keyAction({ key: 't' })).toBe('today');
    expect(keyAction({ key: 'm' })).toBe('tomorrow');
    expect(keyAction({ key: 'n' })).toBe('noDate');
    expect(keyAction({ key: ']', metaKey: true })).toBe('indent');
    expect(keyAction({ key: '[', ctrlKey: true })).toBe('outdent');
    expect(keyAction({ key: 'Delete' })).toBe('delete');
    expect(keyAction({ key: 'a', ctrlKey: true })).toBe('selectAll');
    expect(keyAction({ key: 'Escape' })).toBe('escape');
    expect(keyAction({ key: 'x' })).toBeNull();
    expect(keyAction({ key: 'q', altKey: true })).toBeNull();
    expect(keyAction({ key: 's', ctrlKey: true })).toBeNull();
    expect(keyAction({ key: 'f' })).toBe('filter');
  });

  it('maps board keys (Trello-like), leaving the rest to the list keys', () => {
    expect(boardKeyAction({ key: 'ArrowLeft' })).toBe('left');
    expect(boardKeyAction({ key: 'ArrowRight' })).toBe('right');
    expect(boardKeyAction({ key: 'j' })).toBe('down');
    expect(boardKeyAction({ key: 'Enter' })).toBe('open');
    expect(boardKeyAction({ key: 'n' })).toBe('newCard');
    expect(boardKeyAction({ key: 'e' })).toBe('quickEdit');
    expect(boardKeyAction({ key: 'l' })).toBe('labels');
    expect(boardKeyAction({ key: 'd' })).toBe('due');
    expect(boardKeyAction({ key: 'c' })).toBe('complete');
    expect(boardKeyAction({ key: ' ' })).toBe('toggleSelect');
    expect(boardKeyAction({ key: '<', shiftKey: true })).toBe('moveLeft');
    expect(boardKeyAction({ key: '>', shiftKey: true })).toBe('moveRight');
    expect(boardKeyAction({ key: ',' })).toBeNull();
    for (const key of ['q', 'f', '1', 't', 'Delete', 'Escape', '/']) expect(boardKeyAction({ key })).toBeNull();
    expect(boardKeyAction({ key: 'Enter', ctrlKey: true })).toBeNull();
  });

  it('moves the cursor across board columns', () => {
    const cols = [['a', 'b', 'c'], [], ['d'], ['e', 'f']];
    expect(boardNav(cols, null, 'down')).toBe('a');
    expect(boardNav([[], []], null, 'down')).toBeNull();
    expect(boardNav(cols, 'a', 'down')).toBe('b');
    expect(boardNav(cols, 'c', 'down')).toBe('c');
    expect(boardNav(cols, 'a', 'up')).toBe('a');
    // Right skips the empty column, to the same position (or the last card).
    expect(boardNav(cols, 'c', 'right')).toBe('d');
    expect(boardNav(cols, 'd', 'right')).toBe('e');
    expect(boardNav(cols, 'b', 'left')).toBe('b');
    expect(boardNav(cols, 'f', 'left')).toBe('d');
    expect(boardNav(cols, 'f', 'right')).toBe('f');
    // A cursor that left the board starts over.
    expect(boardNav(cols, 'gone', 'right')).toBe('a');
  });

  it('moves the cursor and selects ranges', () => {
    const order = ['a', 'b', 'c', 'd'];
    expect(moveCursor(order, null, 1)).toBe('a');
    expect(moveCursor(order, null, -1)).toBe('d');
    expect(moveCursor(order, 'b', 1)).toBe('c');
    expect(moveCursor(order, 'd', 1)).toBe('d');
    expect(moveCursor(order, 'a', -1)).toBe('a');
    expect(moveCursor([], 'a', 1)).toBeNull();
    expect(rangeSelect(order, 'd', 'b')).toEqual(['b', 'c', 'd']);
    expect(rangeSelect(order, null, 'b')).toEqual(['b']);
  });
});

describe('quick add', () => {
  it('splits text at token spans (UTF-16) and drops stale tokens', () => {
    const text = '🚀 Launch tomorrow p1';
    const tokens: QuickToken[] = [
      { kind: 'priority', start: 19, end: 21, text: 'p1', value: 'p1' },
      { kind: 'date', start: 10, end: 18, text: 'tomorrow', value: '2026-10-24' },
    ];
    const segs = segments(text, tokens);
    expect(segs.map((s) => [s.text, s.token?.kind])).toEqual([
      ['🚀 Launch ', undefined],
      ['tomorrow', 'date'],
      [' ', undefined],
      ['p1', 'priority'],
    ]);
    // After an edit the old offsets no longer match: the token is ignored.
    expect(segments('🚀 Launch tomorow p1', tokens).filter((s) => s.token).map((s) => s.token!.kind)).toEqual([]);
    expect(segments('', [])).toEqual([]);
  });

  it('finds the #list / @label / /section being typed', () => {
    expect(sigilAt('Fix #Wo', 7)).toEqual({ sigil: '#', start: 4, partial: 'Wo' });
    expect(sigilAt('@', 1)).toEqual({ sigil: '@', start: 0, partial: '' });
    expect(sigilAt('a /Ba', 5)).toEqual({ sigil: '/', start: 2, partial: 'Ba' });
    expect(sigilAt('mail bob@x', 10)).toBeNull();
    expect(sigilAt('http/users', 10)).toBeNull();
    expect(sigilAt('#Work done', 10)).toBeNull();
    expect(suggest(['Work', 'Side project', 'Homework'], 'wo')).toEqual(['Work', 'Homework']);
    expect(suggest(['Work'], 'work')).toEqual([]);
    expect(suggest(['a', 'b'], '')).toEqual(['a', 'b']);
  });
});

describe('misc', () => {
  it('round-trips view keys', () => {
    for (const v of [{ kind: 'today' }, { kind: 'all' }, { kind: 'list', id: '01ABC' }, { kind: 'label', name: 'a:b' }, { kind: 'filter', id: 'F' }] as const) expect(parseViewKey(viewKey(v))).toEqual(v);
    expect(parseViewKey('list:')).toBeNull();
    expect(parseViewKey('nope')).toBeNull();
  });

  it('colours and links', () => {
    expect(colorVar('red')).toBe('var(--td-red)');
    expect(colorVar('chartreuse')).toBeUndefined();
    expect(colorVar(null)).toBeUndefined();
    expect(colorVar('#FF8800')).toBe('#FF8800');
    expect(colorVar('#abc')).toBe('#abc');
    expect(colorVar('#abcd')).toBeUndefined();
    expect(linkLabel('http/users/list.req.yaml')).toBe('users/list');
    expect(linkLabel('notes/design/auth.md')).toBe('design/auth');
    expect(linkLabel('https://example.com/x')).toBe('example.com/x');
  });
});

describe('label colours', () => {
  it('gives labels the same default colour as Rust (`default_label_color`)', () => {
    for (const [name, color] of [
      ['bug', 'blue'],
      ['api', 'pink'],
      ['waiting-on', 'yellow'],
      ['urgent', 'red'],
      ['verify', 'teal'],
      ['Bug', 'blue'],
      ['@bug', 'blue'],
      ['', 'blue'],
    ])
      expect(defaultLabelColor(name)).toBe(color);
  });

  it('uses a label’s own colour, else its default', () => {
    const labels = [{ name: 'bug', color: 'red' }, { name: 'api' }, { name: 'hex', color: '#ff8800' }, { name: 'odd', color: 'url(x)' }];
    expect(labelColor('bug', labels)).toBe('red');
    expect(labelColor('BUG', labels)).toBe('red');
    expect(labelColor('api', labels)).toBe('pink');
    expect(labelColor('hex', labels)).toBe('#ff8800');
    expect(labelColor('odd', labels)).toBe(defaultLabelColor('odd'));
    // New labels (quick-add preview): the default.
    expect(labelColor('new-one', labels)).toBe(defaultLabelColor('new-one'));
    expect(labelColor('x', null)).toBe(defaultLabelColor('x'));
  });

  it('counts task-list items in Markdown (card checklists)', () => {
    expect(markdownChecklist('')).toEqual({ done: 0, total: 0 });
    expect(markdownChecklist(undefined)).toEqual({ done: 0, total: 0 });
    const md = ['Steps:', '- [ ] one', '- [x] two', '  * [X] nested', '1. [ ] numbered', '+ [ ]', '- [] not one', 'text [ ] no', '```', '- [ ] in code', '```', '~~~md', '- [x] tilde code', '~~~', '- [x] after'].join('\r\n');
    expect(markdownChecklist(md)).toEqual({ done: 3, total: 6 });
  });
});

describe('links', () => {
  it('tells URLs, absolute paths and project paths apart', () => {
    expect(classifyLink('https://github.com/a/b/pull/1')).toEqual({ kind: 'url', url: 'https://github.com/a/b/pull/1' });
    expect(classifyLink('mailto:me@example.com')).toMatchObject({ kind: 'url' });
    // Absolute paths, Windows and POSIX, are revealed as they are.
    expect(classifyLink('C:\\Users\\me\\Downloads\\spec.pdf', 'D:\\work\\shop')).toEqual({ kind: 'file', path: 'C:\\Users\\me\\Downloads\\spec.pdf' });
    expect(classifyLink('D:/logs/app.log')).toEqual({ kind: 'file', path: 'D:/logs/app.log' });
    expect(classifyLink('\\\\server\\share\\x.txt')).toEqual({ kind: 'file', path: '\\\\server\\share\\x.txt' });
    expect(classifyLink('/Users/me/notes.txt', '/work/shop')).toEqual({ kind: 'file', path: '/Users/me/notes.txt' });
    expect(classifyLink('file:///C:/Users/me/a%20b.png')).toEqual({ kind: 'file', path: 'C:/Users/me/a b.png' });
    expect(classifyLink('file:///home/me/x.md')).toEqual({ kind: 'file', path: '/home/me/x.md' });
    // Project-relative paths resolve against the task's project root, with its separator.
    expect(classifyLink('http/users/list.req.yaml', 'D:\\work\\shop')).toEqual({ kind: 'file', path: 'D:\\work\\shop\\http\\users\\list.req.yaml', rel: 'http/users/list.req.yaml' });
    expect(classifyLink('/notes/design/auth.md', '/work/shop/')).toEqual({ kind: 'file', path: '/work/shop/notes/design/auth.md', rel: 'notes/design/auth.md' });
    expect(classifyLink('./docs/readme.md', '/work/shop')).toEqual({ kind: 'file', path: '/work/shop/docs/readme.md', rel: 'docs/readme.md' });
    expect(classifyLink('notes\\x.md', '/work/shop')).toMatchObject({ rel: 'notes/x.md' });
    expect(classifyLink('notes/x.md')).toEqual({ kind: 'unresolved', rel: 'notes/x.md' });
    expect(classifyLink('/notes/x.md')).toEqual({ kind: 'unresolved', rel: 'notes/x.md' });
    // Files in one of the project's repositories, wherever that repo is on this machine.
    expect(classifyLink('repo:api/src/routes.ts', '/work/shop')).toEqual({ kind: 'repo', repo: 'api', path: 'src/routes.ts' });
    expect(classifyLink('repo:web')).toEqual({ kind: 'repo', repo: 'web', path: '' });
  });

  it('finds documents /dev/all opens', () => {
    expect(appDocument('http/users/list.req.yaml')).toEqual({ section: 'http', param: 'users/list.req.yaml' });
    expect(appDocument('http/chat.ws.yaml')).toEqual({ section: 'http', param: 'chat.ws.yaml' });
    expect(appDocument('notes/design/auth.md')).toEqual({ section: 'notes', param: 'design/auth.md' });
    expect(appDocument('notes/lab.ipynb')).toEqual({ section: 'notes', param: 'lab.ipynb' });
    expect(appDocument('mocks/mappings/a.json')).toBeUndefined();
    expect(appDocument('http/_folder.yaml')).toBeUndefined();
    expect(isAbsolutePath('C:\\x') && isAbsolutePath('/x') && isAbsolutePath('\\\\s\\x')).toBe(true);
    expect(isAbsolutePath('http/x') || isAbsolutePath('C:x')).toBe(false);
    expect(joinPath('C:\\a\\', 'b/./c.txt')).toBe('C:\\a\\b\\c.txt');
    expect(joinPath('/a', 'b/c')).toBe('/a/b/c');
  });
});
