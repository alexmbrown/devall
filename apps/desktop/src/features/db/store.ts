import { create } from 'zustand';
import { uid } from '@/lib/id';
import { dbApi, type ExplainOutput, type QueryOutput, type SessionInfo, type TableDetails } from '@/lib/api/db';

/** A SQL editor tab (per connection). */
export interface SqlTab {
  id: string;
  title: string;
  sql: string;
  /** `queries/…` path when bound to a saved query. */
  savedPath?: string;
  /** Text as last saved (dirty tracking). */
  savedText?: string;
  output?: QueryOutput;
  /** Which result set is shown. */
  resultIndex: number;
  /** Set while running. */
  queryId?: string;
  /** Live progress ("statement 3 of 10"). */
  progress?: string;
  error?: string;
  explain?: ExplainOutput;
  /** Opened from the schema tree: rows of one table, editable by primary key. */
  table?: { schema?: string; name: string; details?: TableDetails };
  /** The SQL that produced `output` (for paging). */
  ranSql?: string;
  limit: number;
}

export const DEFAULT_LIMIT = 1000;

export function newSqlTab(partial: Partial<SqlTab> = {}): SqlTab {
  return { id: uid(), title: 'Query', sql: '', resultIndex: 0, limit: DEFAULT_LIMIT, ...partial };
}

/** Connection key: project root + connection file path. */
export const connKey = (root: string, path: string) => `${root}|${path}`;

interface DbStore {
  /** Open sessions by connection key. */
  sessions: Record<string, SessionInfo>;
  /** Environment each session was opened with. */
  sessionEnv: Record<string, string | undefined>;
  connecting: Record<string, boolean>;
  /** SQL tabs by connection key. */
  tabs: Record<string, SqlTab[]>;
  activeTab: Record<string, string>;

  setSession(key: string, info: SessionInfo | undefined, env?: string): void;
  setConnecting(key: string, v: boolean): void;
  /** Re-reads open sessions from the backend (they survive page reloads). */
  syncSessions(root: string): Promise<void>;

  tabsFor(key: string): SqlTab[];
  addTab(key: string, tab: SqlTab): void;
  updateTab(key: string, id: string, fn: (t: SqlTab) => SqlTab): void;
  closeTab(key: string, id: string): void;
  setActiveTab(key: string, id: string): void;
}

export const useDb = create<DbStore>((set, get) => ({
  sessions: {},
  sessionEnv: {},
  connecting: {},
  tabs: {},
  activeTab: {},

  setSession(key, info, env) {
    set((s) => {
      const sessions = { ...s.sessions };
      const sessionEnv = { ...s.sessionEnv };
      if (info) {
        sessions[key] = info;
        sessionEnv[key] = env;
      } else {
        delete sessions[key];
        delete sessionEnv[key];
      }
      return { sessions, sessionEnv };
    });
  },
  setConnecting(key, v) {
    set((s) => ({ connecting: { ...s.connecting, [key]: v } }));
  },
  async syncSessions(root) {
    try {
      const open = await dbApi.sessions();
      set((s) => {
        const sessions: Record<string, SessionInfo> = {};
        const sessionEnv: Record<string, string | undefined> = {};
        // Keep other projects' sessions as they are.
        for (const [k, v] of Object.entries(s.sessions)) if (!k.startsWith(`${root}|`)) sessions[k] = v;
        for (const o of open) {
          if (o.root !== root) continue;
          const k = connKey(o.root, o.path);
          sessions[k] = o.info;
          sessionEnv[k] = o.env;
        }
        return { sessions, sessionEnv };
      });
    } catch {
      /* backend not ready */
    }
  },

  tabsFor(key) {
    return get().tabs[key] ?? [];
  },
  addTab(key, tab) {
    set((s) => ({ tabs: { ...s.tabs, [key]: [...(s.tabs[key] ?? []), tab] }, activeTab: { ...s.activeTab, [key]: tab.id } }));
  },
  updateTab(key, id, fn) {
    set((s) => ({ tabs: { ...s.tabs, [key]: (s.tabs[key] ?? []).map((t) => (t.id === id ? fn(t) : t)) } }));
  },
  closeTab(key, id) {
    set((s) => {
      const list = s.tabs[key] ?? [];
      const idx = list.findIndex((t) => t.id === id);
      const next = list.filter((t) => t.id !== id);
      const active = s.activeTab[key] === id ? (next[Math.min(idx, next.length - 1)]?.id ?? '') : s.activeTab[key];
      return { tabs: { ...s.tabs, [key]: next }, activeTab: { ...s.activeTab, [key]: active } };
    });
  },
  setActiveTab(key, id) {
    set((s) => ({ activeTab: { ...s.activeTab, [key]: id } }));
  },
}));

// ---------- local query history (per project, in localStorage; never committed) ----------

export interface HistoryItem {
  sql: string;
  connection: string;
  at: number;
  ms: number;
  ok: boolean;
  rows?: number;
}

const HISTORY_MAX = 300;
const historyKey = (root: string) => `devall.db.history.${root}`;

export function readHistory(root: string): HistoryItem[] {
  try {
    const raw = localStorage.getItem(historyKey(root));
    return raw ? (JSON.parse(raw) as HistoryItem[]) : [];
  } catch {
    return [];
  }
}

export function pushHistory(root: string, item: HistoryItem) {
  try {
    const list = readHistory(root).filter((h) => !(h.sql === item.sql && h.connection === item.connection));
    list.unshift(item);
    localStorage.setItem(historyKey(root), JSON.stringify(list.slice(0, HISTORY_MAX)));
  } catch {
    /* storage full or unavailable */
  }
}

export function clearHistory(root: string) {
  try {
    localStorage.removeItem(historyKey(root));
  } catch {
    /* ignore */
  }
}
