import { Layers } from 'lucide-react';
import { EnvChip } from '@/components/EnvChip';
import { envChipInfo } from '@/lib/envColor';
import { linkMessage, notifyLinks, type LinkItem, type LinkProvider } from '@/lib/links/registry';
import { NO_PROJECT } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';
import type { EnvironmentFile } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';

/** The environment's colour chip (LOCAL / STAGING / PROD…), or the generic icon when it has none. */
const envIcon = (f: EnvironmentFile) => (envChipInfo(f) ? <EnvChip env={f} /> : <Layers />);

/** The page param of the project variables (see EnvironmentsPage). */
const PROJECT = '__project';

/** `devall:env/<id>` (an environment) and `devall:variables` (the project variables). */
export const envLinks: LinkProvider = {
  kinds: ['env', 'variables'],
  items() {
    const p = useWorkspace.getState().project;
    if (!p) return [];
    return [
      ...p.environments.map<LinkItem>((e) => ({ link: formatLink('env', e.id), kind: 'env', title: e.file.name, detail: `environments/${e.id}.env.yaml`, keywords: e.file.label, icon: envIcon(e.file) })),
      { link: formatLink('variables'), kind: 'variables', title: 'Project variables', detail: 'devall.yaml', icon: <Layers /> },
    ];
  },
  resolve(kind, id) {
    const p = useWorkspace.getState().project;
    if (!p) return NO_PROJECT;
    if (kind === 'variables') return { title: 'Project variables', icon: <Layers />, detail: 'devall.yaml' };
    const e = p.environments.find((x) => x.id === id);
    return e ? { title: e.file.name, icon: envIcon(e.file), detail: `environments/${e.id}.env.yaml`, note: e.file.protected ? 'Protected environment' : undefined } : null;
  },
  open(kind, id, { go }) {
    if (!useWorkspace.getState().project) return linkMessage('Open the project to follow this link');
    go('environments', kind === 'variables' ? PROJECT : id);
  },
  icon: () => <Layers />,
};

useWorkspace.subscribe((s, prev) => {
  if (s.project !== prev.project) notifyLinks();
});
