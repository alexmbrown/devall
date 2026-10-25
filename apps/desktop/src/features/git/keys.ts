/**
 * Keyboard navigation of the review view: j/k next/previous file, n/p next/previous hunk, v
 * mark the file reviewed and go to the next unreviewed one. Pure, so it's testable.
 */
export type ReviewKey = { type: 'file'; delta: 1 | -1 } | { type: 'hunk'; delta: 1 | -1 } | { type: 'review' };

export interface KeyLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  /** The focused element: keys typed into fields never navigate. */
  target?: EventTarget | ElementLike | null;
}

interface ElementLike {
  tagName?: string;
  isContentEditable?: boolean;
  type?: string;
  closest?(sel: string): unknown;
}

/** Typing into a text field (a read-only diff editor doesn't count). */
export function isTyping(el: KeyLike['target']): boolean {
  const target = el as ElementLike | null | undefined;
  if (!target) return false;
  if (target.closest?.('.git-review-diff')) return false;
  const tag = (target.tagName ?? '').toUpperCase();
  if (tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable) return true;
  if (tag === 'INPUT') return !['checkbox', 'radio', 'button'].includes((target.type ?? 'text').toLowerCase());
  return false;
}

export function reviewKey(e: KeyLike): ReviewKey | null {
  if (e.ctrlKey || e.metaKey || e.altKey || isTyping(e.target)) return null;
  switch (e.key) {
    case 'j':
      return { type: 'file', delta: 1 };
    case 'k':
      return { type: 'file', delta: -1 };
    case 'n':
      return { type: 'hunk', delta: 1 };
    case 'p':
      return { type: 'hunk', delta: -1 };
    case 'v':
      return { type: 'review' };
    default:
      return null;
  }
}

/** The item `delta` steps from `current` in a list of `len` (clamped; -1 = none selected yet). */
export function step(len: number, current: number, delta: number): number {
  if (len <= 0) return -1;
  if (current < 0) return delta > 0 ? 0 : len - 1;
  return Math.min(len - 1, Math.max(0, current + delta));
}
