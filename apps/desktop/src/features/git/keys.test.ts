import { describe, expect, it } from 'vitest';
import { gitShortcut, isTyping, reviewKey, step } from './keys';

const el = (tagName: string, over: Record<string, unknown> = {}) => ({ tagName, closest: () => null, ...over });

describe('review keys', () => {
  it('maps j/k/n/p/v', () => {
    expect(reviewKey({ key: 'j' })).toEqual({ type: 'file', delta: 1 });
    expect(reviewKey({ key: 'k' })).toEqual({ type: 'file', delta: -1 });
    expect(reviewKey({ key: 'n' })).toEqual({ type: 'hunk', delta: 1 });
    expect(reviewKey({ key: 'p' })).toEqual({ type: 'hunk', delta: -1 });
    expect(reviewKey({ key: 'v' })).toEqual({ type: 'review' });
    expect(reviewKey({ key: 'x' })).toBeNull();
    expect(reviewKey({ key: 'J' })).toBeNull();
  });

  it('ignores shortcuts and typing in fields, but not a focused read-only diff', () => {
    expect(reviewKey({ key: 'j', ctrlKey: true })).toBeNull();
    expect(reviewKey({ key: 'v', metaKey: true })).toBeNull();
    expect(reviewKey({ key: 'j', target: el('INPUT') })).toBeNull();
    expect(reviewKey({ key: 'j', target: el('TEXTAREA') })).toBeNull();
    expect(reviewKey({ key: 'j', target: el('DIV', { isContentEditable: true }) })).toBeNull();
    expect(reviewKey({ key: 'v', target: el('INPUT', { type: 'checkbox' }) })).toEqual({ type: 'review' });
    expect(isTyping(el('TEXTAREA', { closest: (s: string) => (s === '.git-review-diff' ? {} : null) }))).toBe(false);
    expect(reviewKey({ key: 'n', target: el('BUTTON') })).toEqual({ type: 'hunk', delta: 1 });
  });

  it('steps through a list without running off its ends', () => {
    expect(step(0, -1, 1)).toBe(-1);
    expect(step(3, -1, 1)).toBe(0);
    expect(step(3, -1, -1)).toBe(2);
    expect(step(3, 1, 1)).toBe(2);
    expect(step(3, 2, 1)).toBe(2);
    expect(step(3, 0, -1)).toBe(0);
  });
});

describe('git shortcuts', () => {
  const input = { tagName: 'TEXTAREA' };
  it('maps GitKraken-like keys', () => {
    expect(gitShortcut({ key: 'z', ctrlKey: true })).toBe('undo');
    expect(gitShortcut({ key: 'Z', ctrlKey: true, shiftKey: true })).toBe('redo');
    expect(gitShortcut({ key: 'y', ctrlKey: true })).toBe('redo');
    expect(gitShortcut({ key: 'y', metaKey: true }, true)).toBeNull();
    expect(gitShortcut({ key: 'z', metaKey: true }, true)).toBe('undo');
    expect(gitShortcut({ key: 'z', ctrlKey: true }, true)).toBeNull();
    expect(gitShortcut({ key: 's' })).toBe('stage');
    expect(gitShortcut({ key: 'u' })).toBe('unstage');
    expect(gitShortcut({ key: 'S', ctrlKey: true, shiftKey: true })).toBe('stageAll');
    expect(gitShortcut({ key: 'U', ctrlKey: true, shiftKey: true })).toBe('unstageAll');
    expect(gitShortcut({ key: 'l', ctrlKey: true })).toBe('fetch');
    expect(gitShortcut({ key: 'L', ctrlKey: true, shiftKey: true })).toBe('pull');
    expect(gitShortcut({ key: 'P', ctrlKey: true, shiftKey: true })).toBe('push');
    expect(gitShortcut({ key: 'b', ctrlKey: true })).toBe('newBranch');
    expect(gitShortcut({ key: 'H', ctrlKey: true, shiftKey: true })).toBe('fileHistory');
    expect(gitShortcut({ key: 's', ctrlKey: true, altKey: true })).toBe('stash');
    expect(gitShortcut({ key: 'x' })).toBeNull();
  });
  it('leaves typing alone', () => {
    expect(gitShortcut({ key: 'z', ctrlKey: true, target: input })).toBeNull();
    expect(gitShortcut({ key: 's', target: input })).toBeNull();
  });
});
