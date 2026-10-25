import type { BulkResult, Repo, RepoBranches, RepoSummary } from '@/lib/api/git';

/**
 * A project is "single-repo" when its only repository is the one its folder lives in — source
 * control then looks exactly as it always did. Anything else (listed repos, nested ones found
 * under the folder) shows the repository list.
 */
export function isMultiRepo(repos: Repo[]): boolean {
  return repos.length > 1 || repos.some((r) => r.origin !== 'containing');
}

export interface RepoGroup {
  /** `null` for repositories without a group. */
  group: string | null;
  repos: Repo[];
}

/** Repositories by `group`: ungrouped ones first, then groups in the order they first appear. */
export function groupRepos(repos: Repo[]): RepoGroup[] {
  const out: RepoGroup[] = [];
  const ungrouped = repos.filter((r) => !r.group?.trim());
  if (ungrouped.length) out.push({ group: null, repos: ungrouped });
  for (const r of repos) {
    const g = r.group?.trim();
    if (!g) continue;
    const existing = out.find((x) => x.group?.toLowerCase() === g.toLowerCase());
    if (existing) existing.repos.push(r);
    else out.push({ group: g, repos: [r] });
  }
  return out;
}

/** Tab label: the branch for single-repo projects, `repo (branch)` otherwise, "All repositories" for the overview. */
export function gitTabLabel(multi: boolean, repo: string | null, branch: string | null | undefined): string | null {
  if (!multi) return branch ?? null;
  if (!repo) return 'All repositories';
  return branch ? `${repo} (${branch})` : repo;
}

/** `↑2 ↓1`, `in sync`, `no upstream` — or null when there's no status. */
export function upstreamLabel(s: Pick<RepoSummary, 'upstream' | 'ahead' | 'behind'> | null | undefined): string | null {
  if (!s) return null;
  if (!s.upstream) return 'no upstream';
  if (!s.ahead && !s.behind) return 'in sync';
  return [s.ahead ? `↑${s.ahead}` : '', s.behind ? `↓${s.behind}` : ''].filter(Boolean).join(' ');
}

/** What switching `repo` to `branch` will do, from its branch lists. */
export type SwitchPlan = 'already' | 'local' | 'remote' | 'create' | 'skip' | 'unknown';

export function switchPlan(b: RepoBranches | undefined, branch: string, create: boolean): SwitchPlan {
  const name = branch.trim();
  if (!b?.branches) return b ? 'skip' : 'unknown';
  if (!name) return 'unknown';
  if (b.branches.current === name) return 'already';
  if (b.branches.local.includes(name)) return 'local';
  if (b.branches.remote.includes(name)) return 'remote';
  return create ? 'create' : 'skip';
}

/** Every branch name across repos (local and remote), most common first, then alphabetical. */
export function branchUnion(list: RepoBranches[]): string[] {
  const counts = new Map<string, number>();
  for (const b of list) {
    if (!b.branches) continue;
    for (const n of new Set([...b.branches.local, ...b.branches.remote])) counts.set(n, (counts.get(n) ?? 0) + 1);
  }
  return [...counts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([n]) => n);
}

/** "3 succeeded, 1 failed" for a bulk run's toast. */
export function bulkSummary(results: BulkResult[]): string {
  const ok = results.filter((r) => r.ok).length;
  const failed = results.length - ok;
  return failed ? `${ok} succeeded, ${failed} failed` : `${ok} succeeded`;
}

/** Group suggested for a nested repo found at `frontend/web`: its parent folder's name. */
export function suggestedGroup(relPath: string | null | undefined): string {
  const parts = (relPath ?? '').replace(/\\/g, '/').split('/').filter(Boolean);
  if (parts.length < 2) return '';
  const g = parts[parts.length - 2];
  return g.charAt(0).toUpperCase() + g.slice(1);
}
