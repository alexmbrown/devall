import { create } from 'zustand';

/** Sub-pages of the mock server module (the app's own nav selects the module itself). */
export type Section =
  | 'dashboard'
  | 'stubs'
  | 'graphql'
  | 'websocket'
  | 'grpc'
  | 'mqtt'
  | 'requests'
  | 'near-misses'
  | 'recording'
  | 'scenarios'
  | 'files'
  | 'verify'
  | 'compare'
  | 'drift'
  | 'import'
  | 'settings'
  | 'servers';

export const SECTIONS: Section[] = ['dashboard', 'stubs', 'graphql', 'websocket', 'grpc', 'mqtt', 'requests', 'near-misses', 'recording', 'scenarios', 'files', 'verify', 'compare', 'drift', 'import', 'settings', 'servers'];

interface NavState {
  section: Section;
  /** Free-form sub-route, e.g. a request id. */
  param?: string;
  /** Page filters, e.g. requests `from` / `to` (epoch ms). */
  query: Record<string, string>;
  go(section: Section, param?: string, query?: Record<string, string>): void;
}

const KEY = 'devall.mocks.section';

function initial(): Section {
  try {
    const s = localStorage.getItem(KEY) as Section | null;
    return s && SECTIONS.includes(s) ? s : 'dashboard';
  } catch {
    return 'dashboard';
  }
}

export const useNav = create<NavState>((set) => ({
  section: initial(),
  param: undefined,
  query: {},
  go(section, param, query = {}) {
    set({ section, param, query });
    try {
      localStorage.setItem(KEY, section);
    } catch {
      /* storage unavailable */
    }
  },
}));

/**
 * Deep link from the app's navigation param (`useNav.getState().go('mocks', 'requests/<id>')`,
 * e.g. from the command palette) to a sub-page.
 */
export function parseDeepLink(param?: string): { section: Section; param?: string } | undefined {
  if (!param) return undefined;
  const [s, ...rest] = param.split('/');
  if (!SECTIONS.includes(s as Section)) return undefined;
  return { section: s as Section, param: rest.length ? rest.join('/') : undefined };
}
