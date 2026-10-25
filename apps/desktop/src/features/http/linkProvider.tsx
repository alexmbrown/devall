import { Folder, Send } from 'lucide-react';
import { MethodBadge } from '@/components/ui/misc';
import { linkMessage, notifyLinks, type LinkItem, type LinkProvider } from '@/lib/links/registry';
import { NO_PROJECT } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';
import type { TreeNode } from '@/lib/types';
import { docParam, findTreeNode, useWorkspace } from '@/store/workspace';

function walk(n: TreeNode | null, visit: (n: TreeNode) => void) {
  if (!n) return;
  visit(n);
  if (n.kind === 'folder') n.children.forEach((c) => walk(c, visit));
}

/**
 * `devall:request/<path under http/>` (requests and WebSocket/gRPC/MQTT/Socket.IO files) and
 * `devall:folder/<path>` (a folder's settings), resolved against the open project's tree.
 */
export const requestLinks: LinkProvider = {
  kinds: ['request', 'folder'],
  items() {
    const out: LinkItem[] = [];
    walk(useWorkspace.getState().tree, (n) => {
      if (n.kind === 'request') out.push({ link: formatLink('request', n.path), kind: 'request', title: n.name, detail: n.path, keywords: n.method, icon: <MethodBadge method={n.method} short /> });
      else if (n.path) out.push({ link: formatLink('folder', n.path), kind: 'folder', title: n.name, detail: `${n.path}/`, icon: <Folder /> });
    });
    return out;
  },
  resolve(kind, id) {
    const { project, tree } = useWorkspace.getState();
    if (!project) return NO_PROJECT;
    if (!tree) return undefined;
    const n = findTreeNode(tree, id);
    if (kind === 'folder') return n?.kind === 'folder' ? { title: n.name, icon: <Folder />, detail: `http/${n.path}/` } : null;
    return n?.kind === 'request' ? { title: n.name, icon: <MethodBadge method={n.method} short />, detail: `http/${n.path}` } : null;
  },
  async open(kind, id, { go }) {
    const ws = useWorkspace.getState();
    if (!ws.project) return linkMessage('Open the project to follow this link');
    const param = kind === 'folder' ? docParam({ kind: 'folder', path: id }) : id;
    if (await ws.ensureDoc(param)) go('http', param, kind === 'folder' ? { preview: false } : undefined);
  },
  icon: (kind) => (kind === 'folder' ? <Folder /> : <Send />),
};

useWorkspace.subscribe((s, prev) => {
  if (s.tree !== prev.tree || s.project?.root !== prev.project?.root) notifyLinks();
});
