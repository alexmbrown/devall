import { Binary } from 'lucide-react';
import { pluginOps, usePluginCatalog } from '@/lib/api/plugins';
import { utilsApi, type OpInfo } from '@/lib/api/utils';
import { notifyLinks, type LinkProvider } from '@/lib/links/registry';
import { formatLink } from '@/lib/links/syntax';

let builtin: OpInfo[] | null = null;
let loading = false;

/** Built-in utilities (loaded once) plus the ones installed or trusted plugins add (`<plugin>:<op>`). */
function ops(): OpInfo[] | undefined {
  if (!builtin && !loading) {
    loading = true;
    utilsApi
      .list()
      .then((o) => (builtin = o))
      .catch(() => (builtin = []))
      .finally(() => {
        loading = false;
        notifyLinks();
      });
  }
  return builtin ? [...builtin, ...pluginOps(usePluginCatalog.getState().catalog)] : undefined;
}

/** `devall:utility/<op id>` (e.g. `jwt.decode`). */
export const utilityLinks: LinkProvider = {
  kinds: ['utility'],
  items: () => (ops() ?? []).map((o) => ({ link: formatLink('utility', o.id), kind: 'utility', title: o.name, detail: `${o.category} · ${o.id}`, keywords: o.description, icon: <Binary /> })),
  resolve(_kind, id) {
    const list = ops();
    if (!list) return undefined;
    const o = list.find((x) => x.id === id);
    return o ? { title: o.name, detail: `${o.category} · ${o.id}`, note: o.description } : null;
  },
  open(_kind, id, { go }) {
    go('utilities', id);
  },
  icon: () => <Binary />,
};

usePluginCatalog.subscribe((s, prev) => {
  if (s.catalog !== prev.catalog) notifyLinks();
});
