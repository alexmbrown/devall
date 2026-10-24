import type { DownloadState } from '@/lib/api/ai';

// Pure mapping from a live download + the files on disk to what a row shows (unit-tested).

export const isActive = (phase: DownloadState['phase']) => phase === 'starting' || phase === 'downloading' || phase === 'verifying' || phase === 'installing';

/** What a model / runtime row should offer, from the live download and the files on disk. */
export type DownloadView =
  | { kind: 'active'; phase: 'starting' | 'downloading' | 'verifying' | 'installing'; pct: number; downloaded: number; total: number | null; rate: number }
  | { kind: 'ready' }
  | { kind: 'resume'; pct: number; error?: string }
  | { kind: 'download'; error?: string };

export function downloadView(live: DownloadState | undefined, file: { downloaded: boolean; partialBytes: number; sizeBytes: number | null }): DownloadView {
  if (live && isActive(live.phase)) {
    const total = live.total ?? file.sizeBytes;
    const pct = total ? Math.min(100, (live.downloaded / total) * 100) : 0;
    return { kind: 'active', phase: live.phase as 'starting' | 'downloading' | 'verifying' | 'installing', pct, downloaded: live.downloaded, total, rate: live.bytesPerSec };
  }
  if (file.downloaded) return { kind: 'ready' };
  const error = live?.phase === 'error' ? (live.message ?? 'Download failed') : undefined;
  // After a pause the backend's count is fresher than the catalog's file size we fetched earlier.
  const partial = Math.max(file.partialBytes, live && live.phase === 'paused' ? live.downloaded : 0);
  if (partial > 0 && file.sizeBytes) return { kind: 'resume', pct: Math.min(99, Math.round((partial / file.sizeBytes) * 100)), error };
  return { kind: 'download', error };
}
