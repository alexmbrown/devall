import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';
import { defaultSidebar, normalizeSidebar, type SidebarLayout } from '@/lib/sections';

export type Theme = 'system' | 'light' | 'dark';
export type Density = 'compact' | 'comfortable';
/** Request/response arrangement: side by side, stacked, or by the editor's width. */
export type ReqLayout = 'auto' | 'columns' | 'rows';

/** Shared width of every page's left list/tree pane (`SidePane`). */
export const SIDE_PANE = { min: 240, default: 280, max: 560 } as const;
/** Width of the to-do detail panel (right side). */
export const TODO_DETAIL = { min: 320, default: 420, max: 900 } as const;

interface PrefsState {
  theme: Theme;
  density: Density;
  wordWrap: boolean;
  sidebarCollapsed: boolean;
  /** Width of the left list/tree pane, the same on every page. */
  sidePaneWidth: number;
  /** Left list/tree panes collapsed to a rail (narrow split views collapse on their own). */
  sidePaneCollapsed: boolean;
  /** Width of the request pane when side by side (the response pane takes the rest). */
  requestPaneWidth: number;
  /** Height of the request pane when stacked above the response. */
  requestPaneHeight: number;
  reqLayout: ReqLayout;
  /** Width of the to-do detail panel. */
  todoDetailWidth: number;
  /** Save request edits to disk automatically (debounced). */
  autoSave: boolean;
  /** Last opened project root, reopened on launch. */
  lastProject: string | null;
  /** Active environment per project root. */
  activeEnv: Record<string, string>;
  /** Read the clipboard when the window regains focus on the Utilities page and offer to decode it. */
  clipboardDetect: boolean;
  /** Sidebar order, groups and hidden sections. */
  sidebar: SidebarLayout;
  set<K extends keyof Omit<PrefsState, 'set'>>(key: K, value: PrefsState[K]): void;
}

export const usePrefs = create<PrefsState>()(
  persist(
    (set) => ({
      theme: 'system',
      density: 'compact',
      wordWrap: true,
      sidebarCollapsed: false,
      sidePaneWidth: SIDE_PANE.default,
      sidePaneCollapsed: false,
      requestPaneWidth: 560,
      requestPaneHeight: 320,
      reqLayout: 'auto',
      todoDetailWidth: TODO_DETAIL.default,
      autoSave: true,
      lastProject: null,
      activeEnv: {},
      clipboardDetect: true,
      sidebar: defaultSidebar(),
      set: (key, value) => set({ [key]: value } as Partial<PrefsState>),
    }),
    {
      name: 'devall.prefs',
      version: 2,
      storage: createJSONStorage(() => localStorage),
      // v1 → v2: the request tree width becomes the shared side-pane width.
      migrate: (persisted, version) => {
        const p = (persisted ?? {}) as Record<string, unknown>;
        if (version < 2 && typeof p.treeWidth === 'number') p.sidePaneWidth = p.treeWidth;
        delete p.treeWidth;
        return p as unknown as PrefsState;
      },
      merge: (persisted, current) => {
        const p = (persisted ?? {}) as Partial<PrefsState>;
        const width = typeof p.sidePaneWidth === 'number' ? Math.min(SIDE_PANE.max, Math.max(SIDE_PANE.min, p.sidePaneWidth)) : current.sidePaneWidth;
        const detail = typeof p.todoDetailWidth === 'number' ? Math.min(TODO_DETAIL.max, Math.max(TODO_DETAIL.min, p.todoDetailWidth)) : current.todoDetailWidth;
        return { ...current, ...p, sidePaneWidth: width, todoDetailWidth: detail, sidebar: normalizeSidebar(p.sidebar) };
      },
    },
  ),
);

export function resolveTheme(t: Theme): 'light' | 'dark' {
  if (t !== 'system') return t;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}
