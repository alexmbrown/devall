import { create } from 'zustand';
import { listen } from '@tauri-apps/api/event';
import { toast } from '@/components/ui/toast';
import { aiApi, DOWNLOAD_EVENT, type DownloadState } from '@/lib/api/ai';
import { errorMessage } from '@/lib/ipc';
import { isActive } from './downloadState';

export { downloadView, isActive, type DownloadView } from './downloadState';

/**
 * Model and llama.cpp downloads, app-wide. The backend owns them (they keep running when the AI
 * page unmounts) and broadcasts every change as an `ai://download` event; this store subscribes
 * once at app start and is re-hydrated from `ai_downloads`, so any view mounting mid-download
 * shows the live percentage instead of a stale "Resume".
 */
interface DownloadsStore {
  /** Active downloads plus the last terminal state of each key (until the next start). */
  byKey: Record<string, DownloadState>;
  /** Display names for toasts, by key. */
  labels: Record<string, string>;
  /** Bumped when a download ends, so catalog views refresh file status. */
  finished: number;
  apply(s: DownloadState): void;
  hydrate(): Promise<void>;
  /** Starts (or re-attaches to) a download. */
  start(key: string, label: string, run: () => Promise<DownloadState>): Promise<void>;
  pause(key: string): Promise<void>;
}

export const useAiDownloads = create<DownloadsStore>((set, get) => ({
  byKey: {},
  labels: {},
  finished: 0,
  apply(s) {
    const prev = get().byKey[s.key];
    set((st) => ({ byKey: { ...st.byKey, [s.key]: s }, finished: isActive(s.phase) ? st.finished : st.finished + 1 }));
    // Toast once per transition into a terminal phase.
    if (prev && !isActive(prev.phase)) return;
    const label = get().labels[s.key] ?? (s.key.startsWith('runtime:') ? 'llama.cpp' : s.key);
    if (s.phase === 'done') toast.success(s.key.startsWith('runtime:') ? 'llama.cpp installed' : `${label} downloaded`);
    else if (s.phase === 'paused') toast.info(`${label} paused`, 'It resumes where it stopped next time.');
    else if (s.phase === 'error') toast.error(`Downloading ${label} failed`, s.message ?? undefined);
  },
  async hydrate() {
    try {
      const list = await aiApi.downloads();
      set((st) => {
        const byKey = { ...st.byKey };
        // Anything we thought was active but the backend no longer runs ended while we weren't listening.
        for (const [k, v] of Object.entries(byKey)) if (isActive(v.phase) && !list.some((l) => l.key === k)) delete byKey[k];
        for (const l of list) byKey[l.key] = l;
        return { byKey };
      });
    } catch {
      /* AI module unavailable */
    }
  },
  async start(key, label, run) {
    set((st) => ({ labels: { ...st.labels, [key]: label } }));
    try {
      const s = await run();
      // The backend returns the live state when it was already running — no error, just re-attach.
      const cur = get().byKey[s.key];
      if (!cur || !isActive(cur.phase) || s.downloaded >= cur.downloaded) set((st) => ({ byKey: { ...st.byKey, [s.key]: s } }));
    } catch (e) {
      toast.error(`Could not start ${label}`, errorMessage(e));
    }
  },
  async pause(key) {
    const ok = await aiApi.cancelDownload(key).catch(() => false);
    // Not running on the backend (already ended): refresh what we show.
    if (!ok) await get().hydrate();
  },
}));

let started = false;

/** Subscribes to download events once for the app's lifetime (call at startup). */
export function initAiDownloads() {
  if (started) return;
  started = true;
  void listen<DownloadState>(DOWNLOAD_EVENT, (e) => useAiDownloads.getState().apply(e.payload)).catch(() => {});
  void useAiDownloads.getState().hydrate();
}
