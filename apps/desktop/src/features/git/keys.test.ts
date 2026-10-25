import { describe, expect, it } from 'vitest';
import { isTyping, reviewKey, step } from './keys';

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
