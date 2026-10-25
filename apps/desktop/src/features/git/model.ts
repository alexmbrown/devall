import type { BulkResult, ChangedFile, CompareMode, Comparison, Repo, RepoBranches, RepoSummary } from '@/lib/api/git';

// ---------- the Source control route ----------

/** What a Source control view shows, from its param: `[repo][:compare:<base>...<head>]` or `[repo][:commit:<sha>]`. */
export interface GitRoute {
  /** Repository name (multi-repo projects; single-repo projects may carry it for links). */
  repo: string | null;
  /** A saved comparison: three dots = since the head branched off, two = tip to tip (as in git). */
  compare?: { base: string; head: string; mode: CompareMode };
  /** A commit selected in the graph. */
  commit?: string;
}

/** Parses a Source control param (plain repo names from older layouts included). */
export function parseGitParam(param: string | null | undefined): GitRoute {
  if (!param) return { repo: null };
  const i = param.indexOf(':');
  if (i < 0) return { repo: param };
  const repo = param.slice(0, i) || null;
  const rest = param.slice(i + 1);
  if (rest.startsWith('compare:')) {
    const spec = rest.slice('compare:'.length);
    const three = spec.indexOf('...');
    const two = three < 0 ? spec.indexOf('..') : -1;
    const at = three >= 0 ? three : two;
    if (at > 0) {
      const base = spec.slice(0, at);
      const head = spec.slice(at + (three >= 0 ? 3 : 2));
      if (head) return { repo, compare: { base, head, mode: three >= 0 ? 'mergeBase' : 'direct' } };
    }
    return { repo };
  }
  if (rest.startsWith('commit:') && rest.length > 'commit:'.length) return { repo, commit: rest.slice('commit:'.length) };
  return { repo };
}

/** The param for a route (undefined for the landing page). */
export function formatGitParam(r: GitRoute): string | undefined {
  const repo = r.repo ?? '';
  if (r.compare) return `${repo}:compare:${compareSpec(r.compare.base, r.compare.head, r.compare.mode)}`;
  if (r.commit) return `${repo}:commit:${r.commit}`;
  return repo || undefined;
}

/** `main...feature/x` (since branched) or `main..feature/x` (tip to tip). */
export function compareSpec(base: string, head: string, mode: CompareMode): string {
  return `${base}${mode === 'direct' ? '..' : '...'}${head}`;
}

/** "feature/x → main" — what lands where. */
export function compareTitle(base: string, head: string): string {
  return `${head} → ${base}`;
}

// ---------- compare facts ----------

/** "3 min", "5 h", "2 days" for a number of seconds. */
export function ageText(secs: number): string {
  if (secs < 90) return `${Math.max(1, Math.round(secs))} s`;
  if (secs < 90 * 60) return `${Math.round(secs / 60)} min`;
  if (secs < 36 * 3600) return `${Math.round(secs / 3600)} h`;
  return `${Math.round(secs / 86400)} days`;
}

/** Fetched longer ago than this, a remote base may be behind the server. */
export const STALE_FETCH_SECS = 2 * 3600;

/** The base is a remote branch that was never fetched or not recently. */
export function isStale(c: Pick<Comparison, 'baseRemote' | 'fetchedSecsAgo'>): boolean {
  return !!c.baseRemote && (c.fetchedSecsAgo === null || c.fetchedSecsAgo > STALE_FETCH_SECS);
}

/** Host and path of a remote URL (`git@github.com:acme/web.git`, `https://…`, `ssh://…:2222/…`). */
export function parseRemote(url: string | null | undefined): { host: string; path: string } | null {
  const u = (url ?? '').trim();
  if (!u) return null;
  let m = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/(.+?)(?:\.git)?\/?$/i.exec(u);
  if (!m) m = /^(?:[^@/]+@)?([^/:]+):(?!\/)(.+?)(?:\.git)?\/?$/.exec(u);
  return m ? { host: m[1].toLowerCase(), path: m[2] } : null;
}

const encPath = (s: string) => s.split('/').map(encodeURIComponent).join('/');

/** The host's compare page (GitHub opens the pull request form, GitLab offers "Create merge request"); null for other hosts. */
export function hostCompareUrl(remoteUrl: string | null | undefined, base: string, head: string): string | null {
  const r = parseRemote(remoteUrl);
  if (!r || !base || !head) return null;
  if (r.host.includes('github')) return `https://${r.host}/${r.path}/compare/${encPath(base)}...${encPath(head)}?expand=1`;
  if (r.host.includes('gitlab')) return `https://${r.host}/${r.path}/-/compare/${encPath(base)}...${encPath(head)}`;
  return null;
}

const LIST_CAP = 200;

/** A Markdown summary of a comparison (commits + files) for an MR description or a note. */
export function markdownSummary(c: Comparison, files: ChangedFile[] = c.files): string {
  const add = files.reduce((n, f) => n + (f.added ?? 0), 0);
  const del = files.reduce((n, f) => n + (f.deleted ?? 0), 0);
  const out = [`## ${compareTitle(c.base.name, c.head.name)}`, ''];
  const since = c.mode === 'mergeBase' && c.mergeBase ? `since ${c.mergeBase.slice(0, 7)} (where it branched off)` : `compared tip to tip with ${c.base.short}`;
  out.push(`${c.commits.length}${c.commitsTruncated ? '+' : ''} commit${c.commits.length === 1 ? '' : 's'} · ${files.length} file${files.length === 1 ? '' : 's'} changed (+${add} −${del}) · ${since}${c.uncommitted ? ' · includes uncommitted changes' : ''}`, '');
  if (c.commits.length) {
    out.push('### Commits', '');
    for (const k of c.commits.slice(0, LIST_CAP)) out.push(`- ${k.short} ${k.subject}${k.equivalent ? ' _(already on the base)_' : ''}`);
    if (c.commits.length > LIST_CAP) out.push(`- … and ${c.commits.length - LIST_CAP} more`);
    out.push('');
  }
  if (files.length) {
    out.push('### Files', '');
    for (const f of files.slice(0, LIST_CAP)) {
      const name = f.oldPath && f.oldPath !== f.path ? `\`${f.oldPath}\` → \`${f.path}\`` : `\`${f.path}\``;
      const counts = f.binary ? ' (binary)' : ` (+${f.added ?? 0} −${f.deleted ?? 0})`;
      out.push(`- ${f.status} ${name}${counts}`);
    }
    if (files.length > LIST_CAP) out.push(`- … and ${files.length - LIST_CAP} more`);
  }
  return out.join('\n').trimEnd() + '\n';
}

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
