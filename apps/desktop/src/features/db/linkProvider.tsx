import type { ReactNode } from 'react';
import { Database, FileCode2, FileJson, Terminal } from 'lucide-react';
import { dbApi, type QueryKind } from '@/lib/api/db';
import { linkMessage, type LinkItem, type LinkProvider } from '@/lib/links/registry';
import { listSource, NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';

const connections = listSource({ folders: ['connections/'], load: (root) => dbApi.list(root) });
const queries = listSource({ folders: ['queries/'], load: (root) => dbApi.queries(root) });

const QUERY_ICON: Record<QueryKind, ReactNode> = { sql: <FileCode2 />, redis: <Terminal />, mongo: <FileJson /> };
/** As the saved-queries list names them: the path for nested files, else the query's name. */
const queryTitle = (path: string, name: string) => (path.includes('/') ? path.replace(/\.(sql|redis|mongo\.json)$/, '') : name);

/** `devall:connection/<path under connections/>` and `devall:query/<path under queries/>` (opens on its connection). */
export const dbLinks: LinkProvider = {
  kinds: ['connection', 'query'],
  items: () => [
    ...(connections.get() ?? []).map<LinkItem>((c) => ({ link: formatLink('connection', c.path), kind: 'connection', title: c.name, detail: c.target || c.path, keywords: c.kind, icon: <Database /> })),
    ...(queries.get() ?? []).map<LinkItem>((q) => ({ link: formatLink('query', q.path), kind: 'query', title: queryTitle(q.path, q.name), detail: q.connection ? `${q.path} · ${q.connection}` : q.path, keywords: q.kind, icon: QUERY_ICON[q.kind] })),
  ],
  resolve(kind, id) {
    if (!projectRoot()) return NO_PROJECT;
    if (kind === 'connection') {
      const list = connections.get();
      if (!list) return undefined;
      const c = list.find((x) => x.path === id);
      return c ? { title: c.name, icon: <Database />, detail: `connections/${c.path}${c.target ? ` · ${c.target}` : ''}`, note: c.error } : null;
    }
    const list = queries.get();
    if (!list) return undefined;
    const q = list.find((x) => x.path === id);
    return q ? { title: queryTitle(q.path, q.name), icon: QUERY_ICON[q.kind], detail: `queries/${q.path}${q.connection ? ` · ${q.connection}` : ''}`, note: q.error } : null;
  },
  open(kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    // A saved query opens on its connection (the nonce makes a repeated click open it again).
    go('data', kind === 'query' ? `query:${id}|${Date.now()}` : id);
  },
  icon: (kind) => (kind === 'query' ? <FileCode2 /> : <Database />),
};
