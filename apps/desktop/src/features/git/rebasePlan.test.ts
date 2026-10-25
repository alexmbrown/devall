import { describe, expect, it } from 'vitest';
import type { RebaseTodo } from '@/lib/api/git';
import { actionForKey, defaultMessage, fromSteps, headOf, initialPlan, isNoop, moveRow, needsMessage, planSummary, toSteps, validatePlan, type PlanRow } from './rebasePlan';

const todo: RebaseTodo = {
  base: 'b'.repeat(40),
  baseShort: 'bbbbbbb',
  branch: 'feature',
  hasMerges: false,
  canUpdateRefs: true,
  commits: ['one', 'two', 'three', 'four'].map((s, i) => ({ hash: `${i}`.repeat(40), short: `${i}`.repeat(7), subject: s, body: i === 1 ? 'details' : '', author: 'A', date: '', branches: i === 2 ? ['stack'] : [], merge: false })),
};

const set = (rows: PlanRow[], i: number, action: PlanRow['action']) => rows.map((r, k) => (k === i ? { ...r, action } : r));

describe('rebase plan', () => {
  it('starts as all picks and is a no-op', () => {
    const rows = initialPlan(todo);
    expect(rows.map((r) => r.action)).toEqual(['pick', 'pick', 'pick', 'pick']);
    expect(rows[1].original).toBe('two\n\ndetails');
    expect(isNoop(rows, todo.commits.map((c) => c.hash))).toBe(true);
    expect(isNoop(moveRow(rows, 0, 1), todo.commits.map((c) => c.hash))).toBe(false);
    expect(planSummary(rows)).toBe('4 commits: 4 picked');
  });

  it('groups squashes under their head and combines messages', () => {
    let rows = initialPlan(todo);
    rows = set(rows, 1, 'squash');
    rows = set(rows, 2, 'drop');
    rows = set(rows, 3, 'fixup');
    expect(headOf(rows, 1)).toBe(0);
    expect(headOf(rows, 3)).toBe(0); // the drop in between doesn't break the group
    expect(needsMessage(rows, 0)).toBe(true);
    expect(needsMessage(rows, 3)).toBe(false);
    expect(defaultMessage(rows, 0)).toBe('one\n\ntwo\n\ndetails');
    const steps = toSteps(rows);
    expect(steps.map((s) => [s.action, s.message ?? null])).toEqual([
      ['pick', 'one\n\ntwo\n\ndetails'],
      ['squash', null],
      ['drop', null],
      ['fixup', null],
    ]);
    expect(planSummary(rows)).toBe('4 commits: 1 picked, 1 squashed, 1 fixed up, 1 dropped');
  });

  it('validates like git', () => {
    let rows = initialPlan(todo);
    expect(validatePlan(rows)).toBeNull();
    expect(validatePlan(set(rows, 0, 'squash'))).toMatch(/first/);
    expect(validatePlan(set(set(rows, 0, 'drop'), 1, 'fixup'))).toMatch(/first/);
    expect(validatePlan(rows.map((r) => ({ ...r, action: 'drop' as const })))).toMatch(/Reset/);
    rows = set(rows, 2, 'reword');
    expect(validatePlan(rows)).toBeNull(); // defaults to the original message
    rows = rows.map((r, i) => (i === 2 ? { ...r, message: '  ' } : r));
    expect(validatePlan(rows)).toMatch(/needs a message/);
  });

  it('reorders and applies autosquash steps', () => {
    const rows = initialPlan(todo);
    const moved = moveRow(rows, 3, 0);
    expect(moved.map((r) => r.subject)).toEqual(['four', 'one', 'two', 'three']);
    expect(moveRow(rows, 0, 9)).toBe(rows);
    const edited = rows.map((r, i) => (i === 1 ? { ...r, message: 'kept' } : r));
    const after = fromSteps(edited, [
      { action: 'pick', hash: rows[1].hash },
      { action: 'fixup', hash: rows[3].hash },
      { action: 'pick', hash: rows[0].hash },
      { action: 'pick', hash: rows[2].hash },
    ]);
    expect(after.map((r) => [r.subject, r.action])).toEqual([
      ['two', 'pick'],
      ['four', 'fixup'],
      ['one', 'pick'],
      ['three', 'pick'],
    ]);
    expect(after[0].message).toBe('kept');
    expect(actionForKey('S')).toBe('squash');
    expect(actionForKey('x')).toBeNull();
  });
});
