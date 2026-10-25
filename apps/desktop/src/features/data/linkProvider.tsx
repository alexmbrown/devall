import { Dices } from 'lucide-react';
import { dataApi } from '@/lib/api/data';
import { linkMessage, type LinkProvider } from '@/lib/links/registry';
import { listSource, NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';

const schemas = listSource({ folders: ['data/'], load: (root) => dataApi.list(root) });

/** `devall:fakedata/<path under data/>`: a fake-data generator schema. */
export const fakeDataLinks: LinkProvider = {
  kinds: ['fakedata'],
  items: () => (schemas.get() ?? []).map((f) => ({ link: formatLink('fakedata', f.path), kind: 'fakedata', title: f.name, detail: f.path, keywords: f.entities.join(' '), icon: <Dices /> })),
  resolve(_kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    const list = schemas.get();
    if (!list) return undefined;
    const f = list.find((x) => x.path === id);
    return f ? { title: f.name, detail: `data/${f.path}`, note: f.error } : null;
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    go('fakedata', id);
  },
  icon: () => <Dices />,
};
