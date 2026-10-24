import { create } from 'zustand';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { logsApi, type Filter, type OpenProgress, type SourceInfo } from '@/lib/api/logs';
import { useNav, type Section } from '@/store/nav';
import { EMPTY_FILTER } from './format';

/** The nav section id of this module (added to `Section` by the integrator). */
export const LOGS_SECTION = 'logs' as string as Section;

export const EVENTS_KEY = 'events';

/** What other modules can open in the viewer. */
export type LogViewerSource =
  | { type: 'file'; path: string }
  /** Several files merged by timestamp. */
  | { type: 'files'; paths: string[] }
  /** Text you already have (container logs, a pasted log, a response body). */
  | { type: 'text'; name: string; text: string }
  /** A stream source opened on the Rust side (`LogsState::open_stream`). */
  | { type: 'handle'; handle: string }
  /** The project event timeline, optionally narrowed. */
  | { type: 'events'; query?: string; correlationId?: string };

export type Tab = 'logs' | 'patterns' | 'sql';

export interface ViewState {
  filter: Filter;
  /** Time preset id (`all`, `15m`, … or `custom`). */
  preset: string;
  tab: Tab;
  follow: boolean;
  sql: string;
}

export interface RecentFile {
  path: string;
  name: string;
  openedAt: number;
}

interface PendingOpen {
  source: LogViewerSource;
  query?: string;
}

interface LogsStore {
  sources: SourceInfo[];
  active: string;
  progress: Record<string, Record<number, OpenProgress['progress']>>;
  /** Bumped when a source's data changed (indexing progress, tail, reset). */
  versions: Record<string, number>;
  views: Record<string, ViewState>;
  recent: RecentFile[];
  pending: PendingOpen | null;

  setActive(key: string): void;
  view(key: string): ViewState;
  updateView(key: string, patch: Partial<ViewState> | ((v: ViewState) => Partial<ViewState>)): void;
  bump(key: string): void;
  openFile(path: string): Promise<string | undefined>;
  openFiles(paths: string[]): Promise<string | undefined>;
  openText(name: string, text: string): Promise<string | undefined>;
  adopt(handle: string): Promise<string | undefined>;
  close(handle: string): Promise<void>;
  refreshInfo(handle: string): Promise<void>;
  forgetRecent(path: string): void;
  request(p: PendingOpen): void;
  takePending(): PendingOpen | null;
}

const RECENT_KEY = 'devall.logs.recent';
const MAX_RECENT = 12;

function loadRecent(): RecentFile[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(v) ? v : [];
  } catch {
    return [];
  }
}

function saveRecent(list: RecentFile[]) {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(list));
  } catch {
    /* storage unavailable */
  }
}

export function defaultView(key: string): ViewState {
  return {
    filter: EMPTY_FILTER,
    preset: 'all',
    tab: 'logs',
    follow: key === EVENTS_KEY,
    sql: key === EVENTS_KEY
      ? "SELECT kind, count(*) AS n, round(avg(duration_ms), 1) AS avg_ms\nFROM events\nGROUP BY kind\nORDER BY n DESC"
      : 'SELECT level, count(*) AS n\nFROM logs\nGROUP BY level\nORDER BY n DESC',
  };
}

function baseName(p: string): string {
  return p.split(/[\\/]/).pop() || p;
}

export const useLogs = create<LogsStore>((set, get) => {
  // Progress arrives every ~100 ms while indexing; refetch views at most once a second.
  const lastBump: Record<string, number> = {};
  const onProgress = (p: OpenProgress) => {
    set((s) => ({ progress: { ...s.progress, [p.handle]: { ...(s.progress[p.handle] ?? {}), [p.file]: p.progress } } }));
    const done = p.progress.phase === 'ready' || p.progress.phase === 'failed';
    if (done || Date.now() - (lastBump[p.handle] ?? 0) > 1000) {
      lastBump[p.handle] = Date.now();
      get().bump(p.handle);
    }
    if (done) {
      if (p.progress.phase === 'failed') toast.error('Could not index log file', p.progress.error);
      void get().refreshInfo(p.handle);
    }
  };

  const added = (info: SourceInfo) => {
    set((s) => ({
      sources: [...s.sources.filter((x) => x.handle !== info.handle), info],
      active: info.handle,
      views: s.views[info.handle] ? s.views : { ...s.views, [info.handle]: defaultView(info.handle) },
    }));
    return info.handle;
  };

  const remember = (path: string) => {
    const list = [{ path, name: baseName(path), openedAt: Date.now() }, ...get().recent.filter((r) => r.path !== path)].slice(0, MAX_RECENT);
    saveRecent(list);
    set({ recent: list });
  };

  return {
    sources: [],
    active: EVENTS_KEY,
    progress: {},
    versions: {},
    views: {},
    recent: loadRecent(),
    pending: null,

    setActive: (key) => set({ active: key }),
    view: (key) => get().views[key] ?? defaultView(key),
    updateView: (key, patch) =>
      set((s) => {
        const cur = s.views[key] ?? defaultView(key);
        const p = typeof patch === 'function' ? patch(cur) : patch;
        return { views: { ...s.views, [key]: { ...cur, ...p } } };
      }),
    bump: (key) => set((s) => ({ versions: { ...s.versions, [key]: (s.versions[key] ?? 0) + 1 } })),

    async openFile(path) {
      try {
        const info = await logsApi.open(path, onProgress);
        remember(path);
        return added(info);
      } catch (e) {
        toast.error('Could not open log file', errorMessage(e));
      }
    },
    async openFiles(paths) {
      if (paths.length === 1) return get().openFile(paths[0]);
      try {
        const info = await logsApi.merge(paths, onProgress);
        paths.forEach(remember);
        return added(info);
      } catch (e) {
        toast.error('Could not merge log files', errorMessage(e));
      }
    },
    async openText(name, text) {
      try {
        return added(await logsApi.openText(name, text));
      } catch (e) {
        toast.error('Could not open text', errorMessage(e));
      }
    },
    async adopt(handle) {
      try {
        return added(await logsApi.info(handle));
      } catch (e) {
        toast.error('Log source not found', errorMessage(e));
      }
    },
    async close(handle) {
      await logsApi.close(handle).catch(() => false);
      set((s) => {
        const sources = s.sources.filter((x) => x.handle !== handle);
        const views = { ...s.views };
        delete views[handle];
        return { sources, views, active: s.active === handle ? (sources.at(-1)?.handle ?? EVENTS_KEY) : s.active };
      });
    },
    async refreshInfo(handle) {
      try {
        const info = await logsApi.info(handle);
        set((s) => ({ sources: s.sources.map((x) => (x.handle === handle ? info : x)) }));
        get().bump(handle);
      } catch {
        /* closed meanwhile */
      }
    },
    forgetRecent(path) {
      const list = get().recent.filter((r) => r.path !== path);
      saveRecent(list);
      set({ recent: list });
    },
    request: (p) => set({ pending: p }),
    takePending() {
      const p = get().pending;
      if (p) set({ pending: null });
      return p;
    },
  };
});

/**
 * Opens something in the log viewer and switches to it. For other modules, e.g. Docker/K8s:
 * `openInLogViewer({ type: 'text', name: 'api (container)', text })`, or after streaming into a
 * Rust-side stream source: `openInLogViewer({ type: 'handle', handle })`.
 */
export function openInLogViewer(source: LogViewerSource, opts?: { query?: string }) {
  useLogs.getState().request({ source, query: opts?.query });
  useNav.getState().go(LOGS_SECTION);
}
