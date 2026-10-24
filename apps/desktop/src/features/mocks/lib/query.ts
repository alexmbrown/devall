/**
 * A small query cache with the subset of the TanStack Query API the WireMock UI uses
 * (`useQuery`, `useQueryClient().invalidateQueries / setQueryData`). Keeping it in-house avoids
 * a dependency for what is essentially "fetch, cache by key, poll, dedupe, invalidate".
 *
 * Semantics worth knowing:
 * - Queries are shared by key; concurrent fetches of one key are deduplicated.
 * - A mounting observer refetches unless the data is fresh (`staleTime`, at least 2 s so that
 *   ten components mounting together cause one request).
 * - `refetchInterval` polls while at least one observer with an interval is mounted and the
 *   window is visible.
 * - On error the previous data is kept (`status` becomes `error`).
 */
import { useEffect, useMemo, useRef, useSyncExternalStore } from 'react';

export type QueryKey = readonly unknown[];
export type QueryStatus = 'pending' | 'error' | 'success';

export interface QueryState<T = unknown> {
  data?: T;
  error: Error | null;
  status: QueryStatus;
  fetching: boolean;
  updatedAt: number;
  errorAt: number;
}

const MIN_FRESH_MS = 2000;

export function hashKey(key: QueryKey): string {
  return JSON.stringify(key);
}

function startsWith(key: QueryKey, prefix: QueryKey): boolean {
  if (prefix.length > key.length) return false;
  return prefix.every((p, i) => JSON.stringify(p) === JSON.stringify(key[i]));
}

function toError(e: unknown): Error {
  if (e instanceof Error) return e;
  return new Error(typeof e === 'string' ? e : JSON.stringify(e));
}

class Query {
  state: QueryState = { error: null, status: 'pending', fetching: false, updatedAt: 0, errorAt: 0 };
  fn?: () => Promise<unknown>;
  retry = 0;
  observers = 0;
  stale = false;
  private promise?: Promise<unknown>;
  private listeners = new Set<() => void>();

  constructor(readonly key: QueryKey) {}

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => void this.listeners.delete(l);
  };

  getSnapshot = () => this.state;

  set(patch: Partial<QueryState>) {
    this.state = { ...this.state, ...patch };
    for (const l of [...this.listeners]) l();
  }

  fetch(): Promise<unknown> {
    if (this.promise) return this.promise;
    const fn = this.fn;
    if (!fn) return Promise.resolve(this.state.data);
    this.set({ fetching: true });
    const run = async (attempt: number): Promise<unknown> => {
      try {
        return await fn();
      } catch (e) {
        if (attempt < this.retry) {
          await new Promise((r) => setTimeout(r, 400 * (attempt + 1)));
          return run(attempt + 1);
        }
        throw e;
      }
    };
    const p = run(0).then(
      (data) => {
        this.stale = false;
        this.set({ data, error: null, status: 'success', fetching: false, updatedAt: Date.now() });
        return data;
      },
      (e) => {
        this.set({ error: toError(e), status: 'error', fetching: false, errorAt: Date.now() });
        throw e;
      },
    );
    this.promise = p.finally(() => {
      this.promise = undefined;
    });
    // Callers that don't await must not produce unhandled rejections.
    this.promise.catch(() => {});
    return this.promise;
  }
}

export class QueryClient {
  private queries = new Map<string, Query>();

  build(key: QueryKey): Query {
    const h = hashKey(key);
    let q = this.queries.get(h);
    if (!q) {
      q = new Query(key);
      this.queries.set(h, q);
    }
    return q;
  }

  /** Marks matching queries stale and refetches the ones that are on screen. */
  invalidateQueries(opts: { queryKey?: QueryKey } = {}): Promise<void> {
    const jobs: Promise<unknown>[] = [];
    for (const q of this.queries.values()) {
      if (opts.queryKey && !startsWith(q.key, opts.queryKey)) continue;
      q.stale = true;
      if (q.observers > 0 && q.fn) jobs.push(q.fetch().catch(() => {}));
    }
    return Promise.all(jobs).then(() => {});
  }

  setQueryData<T>(key: QueryKey, updater: T | ((old: T | undefined) => T | undefined)) {
    const q = this.build(key);
    const next = typeof updater === 'function' ? (updater as (old: T | undefined) => T | undefined)(q.state.data as T | undefined) : updater;
    if (next === undefined) return;
    q.set({ data: next, status: 'success', error: null, updatedAt: Date.now() });
  }

  getQueryData<T>(key: QueryKey): T | undefined {
    return this.queries.get(hashKey(key))?.state.data as T | undefined;
  }

  /** Fetches (or returns fresh cached) data outside React. */
  fetchQuery<T>(opts: { queryKey: QueryKey; queryFn: () => Promise<T>; staleTime?: number }): Promise<T> {
    const q = this.build(opts.queryKey);
    if (q.state.status === 'success' && !q.stale && Date.now() - q.state.updatedAt < (opts.staleTime ?? 0)) return Promise.resolve(q.state.data as T);
    q.fn = opts.queryFn;
    return q.fetch() as Promise<T>;
  }

  removeQueries(opts: { queryKey?: QueryKey } = {}) {
    for (const [h, q] of this.queries) if ((!opts.queryKey || startsWith(q.key, opts.queryKey)) && q.observers === 0) this.queries.delete(h);
  }
}

/** One client for the whole app: the mock UI and helpers mounted elsewhere share its cache. */
export const queryClient = new QueryClient();

export function useQueryClient(): QueryClient {
  return queryClient;
}

export interface UseQueryOptions<TFn, TData> {
  queryKey: QueryKey;
  queryFn: () => Promise<TFn>;
  enabled?: boolean;
  select?: (data: TFn) => TData;
  /** Milliseconds, `false`, or a function of the query state. */
  refetchInterval?: number | false | ((query: { state: QueryState<TFn> }) => number | false);
  staleTime?: number;
  /** Number of retries, or `false`/`true` (= 0 / 2). */
  retry?: number | boolean;
  /** Shown while a new key loads, e.g. `(prev) => prev` keeps the previous data. */
  placeholderData?: (prev: TFn | undefined) => TFn | undefined;
}

export interface UseQueryResult<TData> {
  data: TData | undefined;
  error: Error | null;
  status: QueryStatus;
  isPending: boolean;
  isError: boolean;
  isSuccess: boolean;
  isFetching: boolean;
  dataUpdatedAt: number;
  refetch(): Promise<unknown>;
}

export function useQuery<TFn, TData = TFn>(opts: UseQueryOptions<TFn, TData>): UseQueryResult<TData> {
  const client = queryClient;
  const hash = hashKey(opts.queryKey);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const q = useMemo(() => client.build(opts.queryKey), [hash]);
  q.fn = opts.queryFn as () => Promise<unknown>;
  q.retry = opts.retry === true ? 2 : typeof opts.retry === 'number' ? opts.retry : 0;
  const enabled = opts.enabled !== false;
  const state = useSyncExternalStore(q.subscribe, q.getSnapshot, q.getSnapshot) as QueryState<TFn>;
  const freshMs = Math.max(opts.staleTime ?? 0, MIN_FRESH_MS);

  useEffect(() => {
    if (!enabled) return;
    q.observers++;
    const s = q.state;
    const fresh = s.status === 'success' && !q.stale && Date.now() - s.updatedAt < freshMs;
    const recentError = s.status === 'error' && Date.now() - s.errorAt < MIN_FRESH_MS;
    if (!fresh && !recentError) q.fetch().catch(() => {});
    return () => {
      q.observers--;
    };
  }, [q, enabled, freshMs]);

  const ri = opts.refetchInterval;
  const interval = typeof ri === 'function' ? ri({ state }) : ri;
  useEffect(() => {
    if (!enabled || !interval || interval <= 0) return;
    const t = setInterval(() => {
      if (typeof document !== 'undefined' && document.hidden) return;
      const last = Math.max(q.state.updatedAt, q.state.errorAt);
      if (Date.now() - last >= interval * 0.8) q.fetch().catch(() => {});
    }, interval);
    return () => clearInterval(t);
  }, [q, enabled, interval]);

  // Previous data per observer, for placeholderData.
  const prevRef = useRef<TFn | undefined>(undefined);
  let raw = state.data;
  if (raw !== undefined) prevRef.current = raw;
  else if (opts.placeholderData) raw = opts.placeholderData(prevRef.current);

  const select = opts.select;
  const data = useMemo(() => (raw === undefined ? undefined : select ? select(raw) : (raw as unknown as TData)), [raw, select]);

  return {
    data,
    error: state.error,
    status: raw !== undefined && state.status === 'pending' ? 'success' : state.status,
    isPending: raw === undefined && state.status === 'pending',
    isError: state.status === 'error',
    isSuccess: state.status === 'success' || (raw !== undefined && state.status === 'pending'),
    isFetching: state.fetching,
    dataUpdatedAt: state.updatedAt,
    refetch: () => q.fetch().catch(() => {}),
  };
}
