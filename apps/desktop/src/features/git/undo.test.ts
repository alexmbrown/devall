import { describe, expect, it } from 'vitest';
import { outcomeMessage } from './undo';

describe('undo outcomes', () => {
  it('words each outcome', () => {
    expect(outcomeMessage({ type: 'done', label: 'Commit “x”', kind: 'commit' }, 'undo')).toEqual({ kind: 'success', title: 'Undone: Commit “x”' });
    expect(outcomeMessage({ type: 'done', label: 'Commit “x”', kind: 'commit' }, 'redo')?.title).toBe('Redone: Commit “x”');
    const r = outcomeMessage({ type: 'refused', label: 'Commit “x”', reason: 'abc1234 has been pushed' }, 'undo');
    expect(r).toEqual({ kind: 'error', title: 'Can’t undo “Commit “x””'.replace('’', "'"), detail: 'abc1234 has been pushed' });
    expect(outcomeMessage({ type: 'nothing' }, 'redo')?.title).toBe('Nothing to redo');
    expect(outcomeMessage({ type: 'confirm', label: 'Discard a.txt', reasons: ['files changed since: a.txt', 'HEAD moved'] }, 'undo')?.detail).toBe('files changed since: a.txt; HEAD moved');
  });
});
