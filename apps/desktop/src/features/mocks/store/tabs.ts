import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import type { MethodPattern, StubMapping } from '@/features/mocks/lib/types';
import { uid } from '@/lib/id';

/**
 * Open editor tabs per server. Kept in sessionStorage (survives reloads, not
 * shared across tabs/devices) — the source of truth for stubs is WireMock.
 */
export interface StubTab {
  key: string;
  /** Server id of the stub; undefined for unsaved new stubs. */
  stubId?: string;
  title: string;
  method?: MethodPattern;
  /** Unsaved edits. Undefined = clean. */
  draft?: StubMapping;
  /** JSON text being edited in raw mode when it is not valid JSON yet. */
  draftText?: string;
  /** Snapshot of the server version when the tab was opened / last saved (for conflict detection). */
  baseline?: string;
  isNew?: boolean;
  pinned?: boolean;
}

interface ServerTabs {
  tabs: StubTab[];
  active?: string;
}

interface WorkspaceState {
  byServer: Record<string, ServerTabs>;
  open(sid: string, tab: Omit<StubTab, 'key'> & { key?: string }, opts?: { preview?: boolean }): string;
  close(sid: string, key: string): void;
  closeOthers(sid: string, key: string): void;
  closeAll(sid: string): void;
  activate(sid: string, key: string): void;
  update(sid: string, key: string, patch: Partial<StubTab>): void;
  findByStub(sid: string, stubId: string): StubTab | undefined;
  move(sid: string, from: number, to: number): void;
}

const empty: ServerTabs = { tabs: [] };

export const useStubTabs = create<WorkspaceState>()(
  persist(
    (set, get) => ({
      byServer: {},
      open(sid, tab, opts) {
        const cur = get().byServer[sid] ?? empty;
        const existing = tab.stubId ? cur.tabs.find((t) => t.stubId === tab.stubId) : undefined;
        if (existing) {
          const tabs = opts?.preview ? cur.tabs : cur.tabs.map((t) => (t.key === existing.key ? { ...t, pinned: true } : t));
          set({ byServer: { ...get().byServer, [sid]: { tabs, active: existing.key } } });
          return existing.key;
        }
        const key = tab.key ?? uid();
        let tabs = [...cur.tabs];
        // "Preview" behaviour: single-click replaces the last clean, unpinned preview tab.
        if (opts?.preview) {
          const idx = tabs.findIndex((t) => !t.pinned && !t.draft && !t.isNew && t.key === cur.active);
          if (idx >= 0) {
            tabs[idx] = { ...tab, key };
            set({ byServer: { ...get().byServer, [sid]: { tabs, active: key } } });
            return key;
          }
        }
        const activeIdx = tabs.findIndex((t) => t.key === cur.active);
        tabs.splice(activeIdx >= 0 ? activeIdx + 1 : tabs.length, 0, { ...tab, key, pinned: !opts?.preview });
        if (tabs.length > 30) tabs = tabs.filter((t, i) => i >= tabs.length - 30 || t.draft);
        set({ byServer: { ...get().byServer, [sid]: { tabs, active: key } } });
        return key;
      },
      close(sid, key) {
        const cur = get().byServer[sid] ?? empty;
        const idx = cur.tabs.findIndex((t) => t.key === key);
        const tabs = cur.tabs.filter((t) => t.key !== key);
        let active = cur.active;
        if (active === key) active = tabs[Math.min(idx, tabs.length - 1)]?.key;
        set({ byServer: { ...get().byServer, [sid]: { tabs, active } } });
      },
      closeOthers(sid, key) {
        const cur = get().byServer[sid] ?? empty;
        set({ byServer: { ...get().byServer, [sid]: { tabs: cur.tabs.filter((t) => t.key === key || t.draft), active: key } } });
      },
      closeAll(sid) {
        const cur = get().byServer[sid] ?? empty;
        const tabs = cur.tabs.filter((t) => t.draft);
        set({ byServer: { ...get().byServer, [sid]: { tabs, active: tabs[0]?.key } } });
      },
      activate(sid, key) {
        const cur = get().byServer[sid] ?? empty;
        set({ byServer: { ...get().byServer, [sid]: { ...cur, active: key } } });
      },
      update(sid, key, patch) {
        const cur = get().byServer[sid] ?? empty;
        set({
          byServer: {
            ...get().byServer,
            [sid]: { ...cur, tabs: cur.tabs.map((t) => (t.key === key ? { ...t, ...patch, pinned: patch.draft ? true : (patch.pinned ?? t.pinned) } : t)) },
          },
        });
      },
      findByStub(sid, stubId) {
        return get().byServer[sid]?.tabs.find((t) => t.stubId === stubId);
      },
      move(sid, from, to) {
        const cur = get().byServer[sid] ?? empty;
        const tabs = [...cur.tabs];
        const [t] = tabs.splice(from, 1);
        tabs.splice(to, 0, t);
        set({ byServer: { ...get().byServer, [sid]: { ...cur, tabs } } });
      },
    }),
    { name: 'devall.mocks.tabs', storage: createJSONStorage(() => sessionStorage) },
  ),
);

export function useServerTabs(sid: string): ServerTabs {
  return useStubTabs((s) => s.byServer[sid] ?? empty);
}
