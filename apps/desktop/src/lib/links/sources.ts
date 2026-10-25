import { useFsVersion, useWorkspace } from '@/store/workspace';
import { notifyLinks } from './registry';

/**
 * A project list a link provider resolves against (load tests, schemas, connections…), loaded on
 * first use, reloaded when files in its folders change on disk (app saves included — the watcher
 * reports them) and dropped when another project opens. `get()` is cheap and safe to call while
 * rendering: it returns what's loaded (undefined while loading) and starts the load once.
 */
export interface ListSource<T> {
  /** Items of the open project; undefined while loading or without a project. */
  get(): T[] | undefined;
  /** Loads (or reloads) now; resolves with the items. */
  load(): Promise<T[]>;
}

export function listSource<T>(opts: { folders: string[]; load(root: string): Promise<T[]> }): ListSource<T> {
  let root: string | null = null;
  let data: T[] | undefined;
  let inflight: Promise<T[]> | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const sync = () => {
    const r = useWorkspace.getState().project?.root ?? null;
    if (r !== root) {
      root = r;
      data = undefined;
      inflight = null;
    }
    return r;
  };

  const load = (): Promise<T[]> => {
    const r = sync();
    if (!r) return Promise.resolve([]);
    if (inflight) return inflight;
    const job = opts
      .load(r)
      .catch(() => [] as T[])
      .then((items) => {
        if (root === r && inflight === job) {
          data = items;
          inflight = null;
          notifyLinks();
        }
        return items;
      });
    inflight = job;
    return job;
  };

  // Files of these folders changed: reload what was loaded (debounced; bursts of saves are one reload).
  useFsVersion.subscribe((s, prev) => {
    if (s.v === prev.v || data === undefined || !s.paths.some((p) => opts.folders.some((f) => p.startsWith(f)))) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      inflight = null;
      void load();
    }, 250);
  });
  useWorkspace.subscribe((s, prev) => {
    if ((s.project?.root ?? null) !== (prev.project?.root ?? null) && data !== undefined) {
      sync();
      notifyLinks();
    }
  });

  return {
    get() {
      if (!sync()) return undefined;
      if (data === undefined && !inflight) void load();
      return data;
    },
    load: () => {
      inflight = null;
      return load();
    },
  };
}

/** The open project's root, or null. */
export const projectRoot = () => useWorkspace.getState().project?.root ?? null;

/** What links into a project resolve to while none is open. */
export const NO_PROJECT = { unavailable: 'no project open' };
