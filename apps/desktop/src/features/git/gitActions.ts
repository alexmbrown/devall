import { create } from 'zustand';

/**
 * Source control's palette commands. The page registers them (bound to the repository it shows)
 * while it is open; the command palette lists them under "Git".
 */
export interface GitPaletteAction {
  id: string;
  label: string;
  keywords: string;
  hint?: string;
  run(): void;
}

export const useGitActions = create<{ actions: GitPaletteAction[]; set(a: GitPaletteAction[]): void }>((set) => ({
  actions: [],
  set: (actions) => set({ actions }),
}));
