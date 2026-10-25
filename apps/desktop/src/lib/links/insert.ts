/**
 * Inserting links while typing Markdown: `[[` opens the "Link to…" picker and the pick replaces it
 * with `[Title](devall:…)`. Pure text helpers shared by the note editor (Monaco) and the to-do
 * description/comment fields (textareas).
 */

/** Inside a fenced code block or an inline code span at `pos` (links aren't links there). */
export function inCode(text: string, pos: number): boolean {
  const before = text.slice(0, pos);
  const fences = before.match(/^[ \t]{0,3}(```|~~~)/gm)?.length ?? 0;
  if (fences % 2 === 1) return true;
  const line = before.slice(before.lastIndexOf('\n') + 1);
  return (line.match(/`/g)?.length ?? 0) % 2 === 1;
}

/**
 * `[[` was just typed before `caret`: the range the picked link replaces — the brackets plus the
 * auto-closed `]]` or `]` right after the caret (Monaco closes brackets) — or null. A third `[`,
 * an escaped `\[[` or code don't trigger.
 */
export function wikiTrigger(text: string, caret: number): { from: number; to: number } | null {
  if (caret < 2 || text.slice(caret - 2, caret) !== '[[') return null;
  const prev = text[caret - 3];
  if (prev === '[' || prev === '\\' || inCode(text, caret - 2)) return null;
  // `[[]]` when both brackets auto-closed, `[[]` when only the first one did.
  const close = text.startsWith(']]', caret) ? 2 : text[caret] === ']' ? 1 : 0;
  return { from: caret - 2, to: caret + close };
}

/** Replaces `[from, to)` with `insert`; the caret lands after it. */
export function replaceRange(text: string, from: number, to: number, insert: string): { text: string; caret: number } {
  return { text: text.slice(0, from) + insert + text.slice(to), caret: from + insert.length };
}

interface KeyLike {
  key: string;
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/**
 * The "Link to…" shortcut: Ctrl+Alt+K (Windows/Linux), ⌘⌥K (macOS). Ctrl/Cmd+K is the command
 * palette. AltGr (Ctrl+Alt on Windows) combinations that type a character don't match: the key must
 * be K itself.
 */
export function isLinkShortcut(e: KeyLike, mac: boolean): boolean {
  if (e.shiftKey || !e.altKey) return false;
  if (mac) return e.metaKey && !e.ctrlKey && (e.code === 'KeyK' || e.key.toLowerCase() === 'k');
  return e.ctrlKey && !e.metaKey && e.key.toLowerCase() === 'k';
}

/** Label of the shortcut for tooltips. */
export const linkShortcutLabel = (mac: boolean) => (mac ? '⌘⌥K' : 'Ctrl+Alt+K');
