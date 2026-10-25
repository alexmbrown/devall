import type { LineSelection, PatchHunk, PatchView } from '@/lib/api/git';

/**
 * Line picking for hunk/line staging (pure; the Changes view uses it, tests cover it). The
 * backend rebuilds the patch from the picked lines (devall_core::repos::patch).
 */

/** Selected line indices per hunk. */
export type Picked = Record<number, number[]>;

/** Changed lines (not context) of a hunk, by index. */
const changedLines = (h: PatchHunk) => h.lines.flatMap((l, i) => (l.kind === ' ' ? [] : [i]));

/** The selection to send: whole hunks when every changed line of them is picked. */
export function toSelection(view: PatchView, picked: Picked): LineSelection[] {
  return Object.entries(picked)
    .map(([k, lines]) => ({ hunk: Number(k), lines }))
    .filter((s) => s.lines.length && view.hunks[s.hunk])
    .map((s) => (changedLines(view.hunks[s.hunk]).every((i) => s.lines.includes(i)) ? { hunk: s.hunk, lines: null } : { hunk: s.hunk, lines: [...s.lines].sort((a, b) => a - b) }))
    .sort((a, b) => a.hunk - b.hunk);
}

/** +/− counts of what a selection touches. */
export function selectionCounts(view: PatchView, sel: LineSelection[]): { added: number; deleted: number } {
  let added = 0;
  let deleted = 0;
  for (const s of sel) {
    const h = view.hunks[s.hunk];
    const idx = s.lines ?? changedLines(h);
    for (const i of idx) {
      if (h.lines[i]?.kind === '+') added++;
      else if (h.lines[i]?.kind === '-') deleted++;
    }
  }
  return { added, deleted };
}

/**
 * Toggles a changed line, or — with an anchor in the same hunk (Shift+click) — adds the run of
 * changed lines between the anchor and it. Context lines can't be picked.
 */
export function pickLine(view: PatchView, picked: Picked, hunk: number, line: number, anchor: { hunk: number; line: number } | null): Picked {
  const h = view.hunks[hunk];
  if (!h || !h.lines[line] || h.lines[line].kind === ' ') return picked;
  const cur = new Set(picked[hunk] ?? []);
  if (anchor && anchor.hunk === hunk) {
    const [a, b] = [anchor.line, line].sort((x, y) => x - y);
    for (let i = a; i <= b; i++) if (h.lines[i].kind !== ' ') cur.add(i);
  } else if (cur.has(line)) cur.delete(line);
  else cur.add(line);
  return { ...picked, [hunk]: [...cur] };
}
