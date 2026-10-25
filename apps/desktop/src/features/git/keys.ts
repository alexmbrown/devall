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

/** Source control actions with a GitKraken-like key (see {@link gitShortcut}). */
export type GitShortcut = 'undo' | 'redo' | 'stage' | 'unstage' | 'stageAll' | 'unstageAll' | 'fetch' | 'newBranch' | 'fileHistory' | 'stash' | 'pull' | 'push';

/**
 * Source control keys (the page ignores them while its view is inactive):
 *  Ctrl/Cmd+Z undo · Ctrl/Cmd+Shift+Z / Ctrl+Y redo · S / U stage / unstage the selected file ·
 *  Ctrl/Cmd+Shift+S / U stage / unstage all · Ctrl/Cmd+L fetch · Ctrl/Cmd+B new branch ·
 *  Ctrl/Cmd+Shift+H file history · Ctrl/Cmd+Alt+S stash · Ctrl/Cmd+Shift+L pull · Ctrl/Cmd+Shift+P push.
 * Typing in a field keeps its own undo and letters. Ctrl/Cmd+Enter commits from the message box.
 */
export function gitShortcut(e: KeyLike & { shiftKey?: boolean }, mac = false): GitShortcut | null {
  if (isTyping(e.target)) return null;
  const mod = mac ? !!e.metaKey : !!e.ctrlKey;
  const key = e.key.toLowerCase();
  if (mod && !e.altKey) {
    if (key === 'z') return e.shiftKey ? 'redo' : 'undo';
    if (key === 'y' && !mac && !e.shiftKey) return 'redo';
    if (e.shiftKey && key === 's') return 'stageAll';
    if (e.shiftKey && key === 'u') return 'unstageAll';
    if (e.shiftKey && key === 'h') return 'fileHistory';
    if (e.shiftKey && key === 'l') return 'pull';
    if (e.shiftKey && key === 'p') return 'push';
    if (!e.shiftKey && key === 'l') return 'fetch';
    if (!e.shiftKey && key === 'b') return 'newBranch';
    return null;
  }
  if (mod && e.altKey && !e.shiftKey && (key === 's' || e.key === 'ß')) return 'stash';
  if (e.ctrlKey || e.metaKey || e.altKey) return null;
  if (key === 's' && !e.shiftKey) return 'stage';
  if (key === 'u' && !e.shiftKey) return 'unstage';
  return null;
}

/** The item `delta` steps from `current` in a list of `len` (clamped; -1 = none selected yet). */
export function step(len: number, current: number, delta: number): number {
  if (len <= 0) return -1;
  if (current < 0) return delta > 0 ? 0 : len - 1;
  return Math.min(len - 1, Math.max(0, current + delta));
}
