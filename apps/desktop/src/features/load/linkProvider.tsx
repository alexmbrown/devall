import { Activity } from 'lucide-react';
import { loadApi } from '@/lib/api/load';
import { linkMessage, type LinkProvider } from '@/lib/links/registry';
import { listSource, NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';

const tests = listSource({ folders: ['load/'], load: (root) => loadApi.list(root) });

/** `devall:load/<path under load/>`: a load test definition. */
export const loadLinks: LinkProvider = {
  kinds: ['load'],
  items: () => (tests.get() ?? []).map((t) => ({ link: formatLink('load', t.path), kind: 'load', title: t.name, detail: t.path, icon: <Activity /> })),
  resolve(_kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    const list = tests.get();
    if (!list) return undefined;
    const t = list.find((x) => x.path === id);
    return t ? { title: t.name, detail: `load/${t.path}`, note: t.error } : null;
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    go('load', id);
  },
  icon: () => <Activity />,
};
