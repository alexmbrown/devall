import { Server } from 'lucide-react';
import { MethodBadge } from '@/components/ui/misc';
import { mockApi, type MockStatus } from '@/lib/api/mock';
import { linkMessage, notifyLinks, type LinkItem, type LinkProvider } from '@/lib/links/registry';
import { NO_PROJECT, projectRoot } from '@/lib/links/sources';
import { formatLink } from '@/lib/links/syntax';
import { clientFor, qk } from './lib/queries';
import { queryClient } from './lib/query';
import { displayName, stubId, urlOf } from './lib/stub-utils';
import type { MappingsResponse, StubMapping } from './lib/types';
import { activeTargetOf, builtinTarget, serverKey, useMockTarget, type Target } from './store/target';

/** The project's engine status, for when the mock page hasn't been opened yet this session. */
type Probe = { root: string; status: MockStatus | null; done: boolean };
let probe: Probe | null = null;

/** The server stub links resolve against: the mock page's active target (undefined while finding out). */
function currentTarget(root: string): Target | null | undefined {
  if (useMockTarget.getState().root === root) return activeTargetOf();
  if (probe?.root !== root) {
    const p: Probe = { root, status: null, done: false };
    probe = p;
    mockApi
      .status(root)
      .then((s) => (p.status = s))
      .catch(() => {})
      .finally(() => {
        p.done = true;
        notifyLinks();
      });
  }
  if (!probe.done) return undefined;
  return probe.status?.running ? builtinTarget(probe.status, root) : null;
}

const watched = new Set<string>();
const failed = new Set<string>();

/** The target's stubs from the shared query cache (fetched once when nothing loaded them yet). */
function mappings(t: Target): StubMapping[] | undefined {
  const sid = serverKey(t);
  const key = qk.mappings(sid);
  const data = queryClient.getQueryData<MappingsResponse>(key);
  if (!watched.has(sid)) {
    watched.add(sid);
    queryClient.build(key).subscribe(notifyLinks);
    const client = clientFor(t);
    if (!data && client)
      queryClient
        .fetchQuery({ queryKey: key, queryFn: () => client.mappings(), staleTime: 15_000 })
        .then(() => failed.delete(sid))
        .catch(() => failed.add(sid))
        .finally(notifyLinks);
  }
  return data?.mappings ?? (failed.has(sid) ? [] : undefined);
}

const method = (m: StubMapping) => (typeof m.request?.method === 'string' ? m.request.method : 'ANY');
const stubIcon = (m: StubMapping) => <MethodBadge method={method(m)} short />;

/** `devall:stub/<stub id>`: a stub of the mock server the Mock server page is pointed at. */
export const stubLinks: LinkProvider = {
  kinds: ['stub'],
  items() {
    const root = projectRoot();
    const t = root ? currentTarget(root) : null;
    if (!t?.url) return [];
    return (mappings(t) ?? []).map<LinkItem>((m) => ({ link: formatLink('stub', stubId(m)), kind: 'stub', title: displayName(m), detail: urlOf(m.request ?? {}), keywords: method(m), icon: stubIcon(m) }));
  },
  resolve(_kind, id) {
    const root = projectRoot();
    if (!root) return NO_PROJECT;
    const t = currentTarget(root);
    if (t === undefined) return undefined;
    if (!t?.url) return { unavailable: 'mock server not running' };
    const list = mappings(t);
    if (!list) return undefined;
    if (failed.has(serverKey(t)) && !list.length) return { unavailable: 'mock server not reachable' };
    const m = list.find((x) => stubId(x) === id);
    return m ? { title: displayName(m), icon: stubIcon(m), detail: `${method(m)} ${urlOf(m.request ?? {})} · ${t.name}` } : null;
  },
  open(_kind, id, { go }) {
    if (!projectRoot()) return linkMessage('Open the project to follow this link');
    // The nonce makes a repeated click on the same link select the stub again.
    go('mocks', `stubs/id:${id}|${Date.now()}`);
  },
  icon: () => <Server />,
};

useMockTarget.subscribe((s, prev) => {
  if (s.root !== prev.root || s.status !== prev.status || s.activeId !== prev.activeId || s.servers !== prev.servers) notifyLinks();
});
