import type { RebaseAction, RebaseStep, RebaseTodo } from '@/lib/api/git';

/**
 * The interactive-rebase plan as the dialog edits it: one row per commit (oldest first, the order
 * git replays them), an action each, and a message where one is needed (a reword, or a commit
 * that others are squashed into). Mirrors the checks `devall_core::repos::rebase::build` makes.
 */
export interface PlanRow {
  hash: string;
  short: string;
  subject: string;
  /** The original full message. */
  original: string;
  action: RebaseAction;
  /** The new message (reword / squash group head); `null` = use the default. */
  message: string | null;
  branches: string[];
}

export const ACTIONS: { value: RebaseAction; label: string; key: string; title: string }[] = [
  { value: 'pick', label: 'Pick', key: 'p', title: 'Keep the commit as it is' },
  { value: 'reword', label: 'Reword', key: 'r', title: 'Keep the changes, edit the message' },
  { value: 'edit', label: 'Edit', key: 'e', title: 'Stop after this commit to amend it' },
  { value: 'squash', label: 'Squash', key: 's', title: 'Meld into the commit above, combining messages' },
  { value: 'fixup', label: 'Fixup', key: 'f', title: "Meld into the commit above, discarding this commit's message" },
  { value: 'drop', label: 'Drop', key: 'd', title: 'Remove the commit' },
];

/** p/r/e/s/f/d → action. */
export function actionForKey(key: string): RebaseAction | null {
  return ACTIONS.find((a) => a.key === key.toLowerCase())?.value ?? null;
}

export function initialPlan(todo: RebaseTodo): PlanRow[] {
  return todo.commits.map((c) => ({ hash: c.hash, short: c.short, subject: c.subject, original: c.body ? `${c.subject}\n\n${c.body}` : c.subject, action: 'pick', message: null, branches: c.branches }));
}

const melds = (a: RebaseAction) => a === 'squash' || a === 'fixup';

/** The kept commit row `i` melds into (`-1`: none — it's a head itself or dropped). */
export function headOf(rows: PlanRow[], i: number): number {
  if (!melds(rows[i].action)) return -1;
  for (let k = i - 1; k >= 0; k--) {
    if (rows[k].action === 'drop') continue;
    if (!melds(rows[k].action)) return k;
  }
  return -1;
}

/** Rows that meld into head `h`. */
export function membersOf(rows: PlanRow[], h: number): number[] {
  const out: number[] = [];
  for (let k = h + 1; k < rows.length; k++) {
    if (rows[k].action === 'drop') continue;
    if (!melds(rows[k].action)) break;
    out.push(k);
  }
  return out;
}

/** The row edits a message: a reword, or a head that something is squashed into. */
export function needsMessage(rows: PlanRow[], i: number): boolean {
  const r = rows[i];
  if (r.action === 'reword') return true;
  if (r.action === 'drop' || melds(r.action)) return false;
  return membersOf(rows, i).some((k) => rows[k].action === 'squash');
}

/** What the message box starts with: the original (reword) or the head's and the squashed commits' messages. */
export function defaultMessage(rows: PlanRow[], i: number): string {
  const r = rows[i];
  const squashed = membersOf(rows, i).filter((k) => rows[k].action === 'squash');
  if (!squashed.length) return r.original;
  return [r.original, ...squashed.map((k) => rows[k].original)].join('\n\n');
}

export function messageOf(rows: PlanRow[], i: number): string {
  return rows[i].message ?? defaultMessage(rows, i);
}

/** Why the plan can't run, or null. */
export function validatePlan(rows: PlanRow[]): string | null {
  const kept = rows.filter((r) => r.action !== 'drop');
  if (!kept.length) return 'Dropping every commit would leave the branch at the base — use Reset instead.';
  if (melds(kept[0].action)) return `${kept[0].short} is first: it has no commit above it to ${kept[0].action} into.`;
  for (let i = 0; i < rows.length; i++) if (needsMessage(rows, i) && !messageOf(rows, i).trim()) return `${rows[i].short} needs a message.`;
  return null;
}

/** The steps for `git_rebase_interactive`: messages only where they're used. */
export function toSteps(rows: PlanRow[]): RebaseStep[] {
  return rows.map((r, i) => ({ action: r.action, hash: r.hash, message: needsMessage(rows, i) ? messageOf(rows, i) : null }));
}

/** Moves row `from` to position `to`. */
export function moveRow(rows: PlanRow[], from: number, to: number): PlanRow[] {
  if (from === to || from < 0 || to < 0 || from >= rows.length || to >= rows.length) return rows;
  const next = [...rows];
  const [r] = next.splice(from, 1);
  next.splice(to, 0, r);
  return next;
}

/** Applies steps (e.g. from autosquash) to rows, keeping edited messages. */
export function fromSteps(rows: PlanRow[], steps: RebaseStep[]): PlanRow[] {
  const by = new Map(rows.map((r) => [r.hash, r]));
  return steps.map((s) => ({ ...by.get(s.hash)!, action: s.action })).filter((r) => !!r.hash);
}

/** The plan changes nothing (every commit picked, in order). */
export function isNoop(rows: PlanRow[], original: string[]): boolean {
  return rows.every((r, i) => r.action === 'pick' && r.hash === original[i]);
}

/** "4 commits: 2 picked, 1 squashed, 1 dropped". */
export function planSummary(rows: PlanRow[]): string {
  const counts = new Map<RebaseAction, number>();
  for (const r of rows) counts.set(r.action, (counts.get(r.action) ?? 0) + 1);
  const word: Record<RebaseAction, string> = { pick: 'picked', reword: 'reworded', edit: 'to edit', squash: 'squashed', fixup: 'fixed up', drop: 'dropped' };
  const parts = ACTIONS.filter((a) => counts.get(a.value)).map((a) => `${counts.get(a.value)} ${word[a.value]}`);
  return `${rows.length} commit${rows.length === 1 ? '' : 's'}: ${parts.join(', ')}`;
}
