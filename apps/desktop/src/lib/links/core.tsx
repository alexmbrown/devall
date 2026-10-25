import { GitBranch, Link2 } from 'lucide-react';
import { SectionIcon } from '@/components/layout/sectionIcons';
import { isSection, SECTIONS, sectionMeta } from '@/lib/sections';
import { useRepos } from '@/store/repos';
import { linkMessage, notifyLinks, type LinkItem, type LinkProvider } from './registry';
import { NO_PROJECT, projectRoot } from './sources';
import { formatLink, pageLink, parsePageId } from './syntax';

/** `devall:page/<section>[/<param>]`: any page of the app (the generic fallback). */
export const pageLinks: LinkProvider = {
  kinds: ['page'],
  items: () => SECTIONS.map<LinkItem>((s) => ({ link: pageLink(s.id), kind: 'page', title: s.title ?? s.label, detail: s.id, icon: <SectionIcon section={s.id} /> })),
  resolve(_kind, id) {
    const { section, param } = parsePageId(id);
    if (!isSection(section)) return null;
    const meta = sectionMeta(section);
    return { title: param ? `${meta.label} › ${param}` : (meta.title ?? meta.label), icon: <SectionIcon section={section} />, detail: param ? `${section}/${param}` : section };
  },
  open(_kind, id, { go }) {
    const { section, param } = parsePageId(id);
    if (isSection(section)) go(section, param);
    else linkMessage('Unknown page', id);
  },
  icon: () => <Link2 />,
};

let repoLoad: Promise<unknown> | null = null;
/** Project whose repo list couldn't be read (not retried on every render). */
let failedFor: string | null = null;
/** Reads the repo list once (no git process); concurrent callers share the read. */
function loadRepos(root: string) {
  if (failedFor === root) return;
  repoLoad ??= useRepos
    .getState()
    .load()
    .catch(() => {
      failedFor = root;
      notifyLinks();
    })
    .finally(() => (repoLoad = null));
}

/** `devall:repo/<name>`: one of the project's repositories (opens it in Source control). Files in a repo stay `repo:<name>/<path>`. */
export const repoLinks: LinkProvider = {
  kinds: ['repo'],
  items() {
    const s = useRepos.getState();
    const root = projectRoot();
    if (!root) return [];
    if (!s.loaded) loadRepos(root);
    return s.repos.map<LinkItem>((r) => ({ link: formatLink('repo', r.name), kind: 'repo', title: r.name, detail: r.relPath ?? r.path, keywords: r.group ?? undefined, icon: <GitBranch /> }));
  },
  resolve(_kind, id) {
    const root = projectRoot();
    if (!root) return NO_PROJECT;
    const s = useRepos.getState();
    if (!s.loaded || s.root !== root) {
      if (failedFor === root) return null;
      loadRepos(root);
      return undefined;
    }
    const r = s.repos.find((x) => x.name === id) ?? s.repos.find((x) => x.name.toLowerCase() === id.toLowerCase());
    if (!r) return null;
    return { title: r.name, icon: <GitBranch />, detail: r.relPath ?? r.path, note: r.state === 'found' ? undefined : r.state === 'missing' ? 'not on this machine' : 'not a git repository' };
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    go('git', id);
  },
  icon: () => <GitBranch />,
};

useRepos.subscribe((s, prev) => {
  if (s.repos !== prev.repos || s.loaded !== prev.loaded) notifyLinks();
});
