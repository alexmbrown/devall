import { useMemo } from 'react';
import { useQuery, useQueryClient, queryClient, type QueryKey } from './query';
import { WireMockClient, ApiError, isDevallVersion } from './api';
import { useActiveTarget, useMockTarget, serverKey, BUILTIN_ID, type Target } from '@/features/mocks/store/target';
import { usePrefs } from '@/features/mocks/store/prefs';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import type { StubMapping } from './types';
import { capabilitiesFor, DEFAULT_CAPABILITIES, type Capabilities } from './capabilities';

/** A client for `t`, or undefined while it has no URL (built-in engine stopped). */
export function clientFor(t: Target): WireMockClient | undefined {
  if (!t.url) return undefined;
  const st = useMockTarget.getState();
  const r = t.remote;
  return new WireMockClient({
    url: t.url,
    contextPath: t.contextPath,
    auth: t.auth,
    headers: r?.headers,
    insecureTls: r?.insecureTls,
    root: st.root ?? undefined,
    // The built-in engine answers fast; a remote one may be across a VPN.
    timeoutMs: t.kind === 'builtin' ? 10_000 : 20_000,
  });
}

export function useClientFor(t?: Target) {
  const key = t ? JSON.stringify([t.url, t.contextPath, t.auth, t.remote?.headers, t.remote?.insecureTls]) : '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => (t ? clientFor(t) : undefined), [key]);
}

/**
 * The active target and its client. `server` is the target (named as in the original WireMock UI);
 * `sid` keys caches and tabs.
 */
export function useWm() {
  const target = useActiveTarget();
  const client = useClientFor(target);
  return { target, server: target, client, sid: serverKey(target) };
}

export const qk = {
  all: (sid: string) => ['wm', sid] as const,
  health: (sid: string) => ['wm', sid, 'health'] as const,
  mappings: (sid: string) => ['wm', sid, 'mappings'] as const,
  mapping: (sid: string, id: string) => ['wm', sid, 'mapping', id] as const,
  requests: (sid: string) => ['wm', sid, 'requests'] as const,
  unmatchedNearMisses: (sid: string) => ['wm', sid, 'near-misses'] as const,
  unmatchedStubs: (sid: string) => ['wm', sid, 'unmatched-stubs'] as const,
  scenarios: (sid: string) => ['wm', sid, 'scenarios'] as const,
  recording: (sid: string) => ['wm', sid, 'recording'] as const,
  settings: (sid: string) => ['wm', sid, 'settings'] as const,
  files: (sid: string) => ['wm', sid, 'files'] as const,
  file: (sid: string, p: string) => ['wm', sid, 'file', p] as const,
  capabilities: (sid: string) => ['wm', sid, 'capabilities'] as const,
  chaos: (sid: string) => ['wm', sid, 'chaos'] as const,
};

/**
 * Server version and the features it implies (WireMock 4 vs 3). The built-in engine reports
 * 3.13, so 4.x-only editors stay hidden for it. Until it loads, 3.x behaviour is assumed.
 */
export function useCapabilities(): Capabilities & { loaded: boolean; devall: boolean } {
  const { client, sid } = useWm();
  const q = useQuery({
    queryKey: qk.capabilities(sid),
    queryFn: async () => {
      const version = await client!.version(); // undefined on 2.x (404)
      if (version) return capabilitiesFor(version);
      const spec = await client!.openApiSpec().catch(() => undefined);
      return capabilitiesFor(undefined, spec?.paths ? Object.keys(spec.paths as object) : undefined);
    },
    enabled: !!client,
    staleTime: 5 * 60_000,
    retry: false,
    refetchInterval: (query) => (query.state.status === 'error' ? 10_000 : false),
  });
  return useMemo(() => ({ ...(q.data ?? DEFAULT_CAPABILITIES), loaded: !!q.data, devall: isDevallVersion(q.data?.version) }), [q.data]);
}

export function useHealth() {
  const { client, sid } = useWm();
  return useQuery({
    queryKey: qk.health(sid),
    queryFn: () => client!.ping(),
    enabled: !!client,
    refetchInterval: 10000,
    retry: false,
  });
}

export function useMappings() {
  const { client, sid } = useWm();
  return useQuery({
    queryKey: qk.mappings(sid),
    queryFn: () => client!.mappings(),
    enabled: !!client,
    select: (r) => r.mappings,
    refetchInterval: 15000,
  });
}

export function useRequests(opts: { live?: boolean } = {}) {
  const { client, sid } = useWm();
  const pollMs = usePrefs((s) => s.pollMs);
  const limit = usePrefs((s) => s.journalLimit);
  return useQuery({
    queryKey: [...qk.requests(sid), limit],
    queryFn: () => client!.requests(limit),
    enabled: !!client,
    refetchInterval: opts.live && pollMs > 0 ? pollMs : false,
  });
}

/**
 * Journal entries inside a time window (overview activity), independent of the Requests page's
 * "last N" limit. Uses ?since= so a busy journal still covers the whole window, up to `limit`.
 */
export const ACTIVITY_LIMIT = 1000;
export function useJournalWindow(windowMs: number | undefined) {
  const { client, sid } = useWm();
  const pollMs = usePrefs((s) => s.pollMs);
  return useQuery({
    queryKey: [...qk.requests(sid), 'window', windowMs ?? 'all'],
    queryFn: async () => {
      const since = windowMs ? new Date(Date.now() - windowMs).toISOString() : undefined;
      const r = await client!.requests(ACTIVITY_LIMIT, since);
      return { ...r, capped: r.requests.length >= ACTIVITY_LIMIT };
    },
    enabled: !!client,
    placeholderData: (prev) => prev,
    refetchInterval: pollMs > 0 ? Math.max(pollMs, 10_000) : false,
  });
}

/** Journal from a point in time (Requests page time filter); live only while the window is open-ended or recent. */
export function useJournalSince(fromMs: number | undefined, live: boolean, limit = 2000) {
  const { client, sid } = useWm();
  const pollMs = usePrefs((s) => s.pollMs);
  return useQuery({
    queryKey: [...qk.requests(sid), 'since', fromMs ?? 0, limit],
    queryFn: async () => {
      const r = await client!.requests(limit, new Date(fromMs!).toISOString());
      return { ...r, capped: r.requests.length >= limit };
    },
    enabled: !!client && fromMs !== undefined,
    placeholderData: (prev) => prev,
    refetchInterval: live && pollMs > 0 ? Math.max(pollMs, 5000) : false,
  });
}

export function useUnmatchedNearMisses(live = false) {
  const { client, sid } = useWm();
  const pollMs = usePrefs((s) => s.pollMs);
  return useQuery({
    queryKey: qk.unmatchedNearMisses(sid),
    queryFn: () => client!.unmatchedNearMisses(),
    enabled: !!client,
    refetchInterval: live && pollMs > 0 ? Math.max(pollMs, 3000) : false,
  });
}

export function useUnmatchedStubs(enabled = true) {
  const { client, sid } = useWm();
  return useQuery({
    queryKey: qk.unmatchedStubs(sid),
    queryFn: () => client!.unmatchedMappings(),
    enabled: !!client && enabled,
    select: (r) => r.mappings,
    retry: false,
    refetchInterval: 15000,
  });
}

export function useScenarios() {
  const { client, sid } = useWm();
  return useQuery({ queryKey: qk.scenarios(sid), queryFn: () => client!.scenarios(), enabled: !!client, select: (r) => r.scenarios, refetchInterval: 10000 });
}

export function useRecordingStatus() {
  const { client, sid } = useWm();
  return useQuery({ queryKey: qk.recording(sid), queryFn: () => client!.recordingStatus(), enabled: !!client, refetchInterval: 5000, retry: false });
}

export function useSettings() {
  const { client, sid } = useWm();
  return useQuery({ queryKey: qk.settings(sid), queryFn: () => client!.getSettings(), enabled: !!client });
}

export function useFiles() {
  const { client, sid } = useWm();
  return useQuery({ queryKey: qk.files(sid), queryFn: () => client!.files(), enabled: !!client });
}

/** Chaos settings (built-in /dev/all engine only). */
export function useChaos(enabled: boolean) {
  const { client, sid } = useWm();
  return useQuery({ queryKey: qk.chaos(sid), queryFn: () => client!.chaos(), enabled: !!client && enabled, retry: false });
}

interface WriteOpts {
  /** Shown in the protected-server confirmation and in error toasts. */
  label: string;
  success?: string | ((r: unknown) => string);
  invalidate?: (sid: string) => QueryKey[];
  /** Always ask, even on unprotected servers (destructive ops). */
  confirm?: { title: string; message?: React.ReactNode; confirmText?: string; danger?: boolean };
  silent?: boolean;
}

/** Called after every successful write: keeps the built-in engine's status (stub count) fresh. */
function afterWrite(t: Target) {
  if (t.id !== BUILTIN_ID) return;
  const st = useMockTarget.getState();
  st.markLocalWrite();
  void st.refreshStatus();
}

/**
 * Wraps every write to WireMock: enforces read-only / protected servers,
 * optional confirmation, toasts, and cache invalidation.
 */
export function useWrite() {
  const { client, server, sid } = useWm();
  const qc = useQueryClient();
  return async function run<T>(fn: (c: WireMockClient) => Promise<T>, opts: WriteOpts): Promise<T | undefined> {
    if (!client || !server) return undefined;
    if (server.readOnly) {
      toast.error('Server is read-only', `"${server.name}" is marked read-only in mocks/servers.yaml.`);
      return undefined;
    }
    if (opts.confirm) {
      const ok = await confirmDialog({
        ...opts.confirm,
        typeToConfirm: server.protected && opts.confirm.danger ? server.name : undefined,
        message: (
          <>
            {opts.confirm.message}
            {server.protected && <div className="mt-4 text-warning small">"{server.name}" is a protected server.</div>}
          </>
        ),
      });
      if (!ok) return undefined;
    } else if (server.protected) {
      const ok = await confirmDialog({ title: `${opts.label}?`, message: `"${server.name}" is a protected server. Apply this change?`, confirmText: 'Apply' });
      if (!ok) return undefined;
    }
    try {
      if (server.id === BUILTIN_ID) useMockTarget.getState().markLocalWrite();
      const r = await fn(client);
      afterWrite(server);
      if (!opts.silent && opts.success) toast.success(typeof opts.success === 'function' ? opts.success(r) : opts.success);
      const keys = opts.invalidate?.(sid) ?? [qk.all(sid)];
      await Promise.all(keys.map((k) => qc.invalidateQueries({ queryKey: k })));
      return r;
    } catch (e) {
      toast.error(`${opts.label} failed`, e instanceof ApiError ? e.message : e instanceof Error ? e.message : String(e));
      throw e;
    }
  };
}

export function useInvalidate() {
  const { sid } = useWm();
  const qc = useQueryClient();
  return (key?: (sid: string) => QueryKey) => qc.invalidateQueries({ queryKey: key ? key(sid) : qk.all(sid) });
}

/** Optimistically replace a stub in the cached list (keeps the UI snappy after saves). */
export function useStubCache() {
  const { sid } = useWm();
  const qc = useQueryClient();
  return {
    upsert(stub: StubMapping) {
      qc.setQueryData(qk.mappings(sid), (old: { mappings: StubMapping[]; meta: { total: number } } | undefined) => {
        if (!old) return old;
        const id = stub.id ?? stub.uuid;
        const i = old.mappings.findIndex((m) => (m.id ?? m.uuid) === id);
        const mappings = [...old.mappings];
        if (i >= 0) mappings[i] = stub;
        else mappings.unshift(stub);
        return { ...old, mappings, meta: { total: mappings.length } };
      });
    },
    remove(ids: string[]) {
      qc.setQueryData(qk.mappings(sid), (old: { mappings: StubMapping[]; meta: { total: number } } | undefined) => {
        if (!old) return old;
        const set = new Set(ids);
        const mappings = old.mappings.filter((m) => !set.has((m.id ?? m.uuid) as string));
        return { ...old, mappings, meta: { total: mappings.length } };
      });
    },
  };
}

/** Drops every cached response of a target (e.g. after the built-in engine restarts). */
export function invalidateTarget(t: Target) {
  return queryClient.invalidateQueries({ queryKey: qk.all(serverKey(t)) });
}
