import { create } from 'zustand';
import { persist, createJSONStorage } from 'zustand/middleware';

export type EditorMode = 'form' | 'json';

interface PrefsState {
  editorMode: EditorMode;
  /** Journal/live polling interval in ms; 0 = off. */
  pollMs: number;
  journalLimit: number;
  stubListWidth: number;
  showFolders: boolean;
  set<K extends keyof Omit<PrefsState, 'set'>>(key: K, value: PrefsState[K]): void;
}

/** Mock-module preferences (app-wide ones such as theme and word wrap live in `@/store/prefs`). */
export const usePrefs = create<PrefsState>()(
  persist(
    (set) => ({
      editorMode: 'form',
      pollMs: 3000,
      journalLimit: 200,
      stubListWidth: 340,
      showFolders: true,
      set: (key, value) => set({ [key]: value } as Partial<PrefsState>),
    }),
    { name: 'devall.mocks.prefs', version: 1, storage: createJSONStorage(() => localStorage) },
  ),
);
