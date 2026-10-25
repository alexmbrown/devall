import { GitCommitHorizontal, GitCompare } from 'lucide-react';
import { linkMessage, type LinkItem, type LinkProvider } from '@/lib/links/registry';
import { NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';
import { useRepos } from '@/store/repos';
import { useCompare } from './compareStore';
import { compareSpec, compareTitle, formatGitParam, parseGitParam } from './model';

/** `<repo>/<rest>` → the repo and the rest (a SHA, or `<base>...<head>`). */
function split(id: string): { repo: string; rest: string } | null {
  const i = id.indexOf('/');
  return i > 0 && i < id.length - 1 ? { repo: id.slice(0, i), rest: id.slice(i + 1) } : null;
}

function route(kind: string, id: string): string | undefined {
  const p = split(id);
  if (!p) return undefined;
  return formatGitParam(kind === 'commit' ? { repo: p.repo, commit: p.rest } : parseGitParam(`${p.repo}:compare:${p.rest}`));
}

/**
 * `devall:commit/<repo>/<sha>` (opens the commit in the graph) and
 * `devall:compare/<repo>/<base>...<head>` (opens the comparison; `..` = tip to tip). Titles come
 * from the link itself — no git process to render a chip.
 */
export const gitLinks: LinkProvider = {
  kinds: ['commit', 'compare'],
  items(): LinkItem[] {
    // The comparison open right now, so it can be linked from a note or a to-do.
    const s = useCompare.getState();
    if (!s.spec || !s.data || !projectRoot()) return [];
    const repo = s.repo ?? useRepos.getState().repos.find((r) => r.origin === 'containing')?.name;
    if (!repo) return [];
    return [{ link: formatLink('compare', `${repo}/${compareSpec(s.spec.base, s.spec.head, s.spec.mode)}`), kind: 'compare', title: compareTitle(s.spec.base, s.spec.head), detail: repo, icon: <GitCompare /> }];
  },
  resolve(kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    const p = split(id);
    if (!p) return null;
    const repos = useRepos.getState();
    const known = !repos.loaded || repos.repos.some((r) => r.name === p.repo || r.name.toLowerCase() === p.repo.toLowerCase());
    if (!known) return { unavailable: `no repository “${p.repo}” in this project` };
    if (kind === 'commit') return { title: `Commit ${p.rest.slice(0, 7)}`, icon: <GitCommitHorizontal />, detail: `${p.repo} · ${p.rest}` };
    const r = parseGitParam(`${p.repo}:compare:${p.rest}`);
    if (!r.compare) return null;
    return { title: compareTitle(r.compare.base, r.compare.head), icon: <GitCompare />, detail: `${p.repo} · ${r.compare.mode === 'mergeBase' ? 'since it branched off' : 'tip to tip'}` };
  },
  open(kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    const param = route(kind, id);
    if (!param) return linkMessage('Not a valid link', id);
    go('git', param);
  },
  icon: (kind) => (kind === 'commit' ? <GitCommitHorizontal /> : <GitCompare />),
};
