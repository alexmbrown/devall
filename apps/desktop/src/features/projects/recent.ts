import { create } from 'zustand';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { api, isTauri } from '@/lib/ipc';
import type { RecentProject } from '@/lib/types';
import { useWorkspace } from '@/store/workspace';
import { openOrCreateProject } from './NewProjectDialog';

/**
 * Recently opened projects (newest first) for the top bar's project switcher and the palette —
 * refreshed when the project changes and when the window regains focus.
 */
export const useRecentProjects = create<{ list: RecentProject[]; refresh(): Promise<void> }>((set) => ({
  list: [],
  async refresh() {
    try {
      set({ list: await api.recentProjects() });
    } catch {
      /* keep the last list */
    }
  },
}));

if (isTauri) {
  void useRecentProjects.getState().refresh();
  useWorkspace.subscribe((s, prev) => {
    if (s.project?.root !== prev.project?.root) void useRecentProjects.getState().refresh();
  });
  window.addEventListener('focus', () => void useRecentProjects.getState().refresh());
}

const norm = (p: string) => p.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase();

/** Same folder (case-insensitive, either slash — projects are folders on Windows/macOS). */
export const samePath = (a: string | undefined | null, b: string | undefined | null) => !!a && !!b && norm(a) === norm(b);

/** The last two segments of a project path, to tell same-named projects apart. */
export function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 2 ? `…/${parts.slice(-2).join('/')}` : parts.join('/');
}

/** Opens a project folder picked in the OS dialog (a folder without devall.yaml offers to become one). */
export async function openProjectFolder() {
  const dir = await openDialog({ directory: true, title: 'Open a /dev/all project (folder with devall.yaml)' });
  if (typeof dir === 'string') await openOrCreateProject(dir, true);
}

/** Starts a new project in a folder picked in the OS dialog. */
export async function createProjectFolder() {
  const dir = await openDialog({ directory: true, title: 'Choose a folder for the new project (your service’s git repo, or a folder holding several repos)' });
  if (typeof dir === 'string') await openOrCreateProject(dir, false);
}
