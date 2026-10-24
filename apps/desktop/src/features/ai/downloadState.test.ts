import { describe, expect, it } from 'vitest';
import type { DownloadState } from '@/lib/api/ai';
import { downloadView, isActive } from './downloadState';

const live = (p: Partial<DownloadState>): DownloadState => ({ key: 'm', phase: 'downloading', downloaded: 0, total: 1000, bytesPerSec: 0, message: null, ...p });
const file = { downloaded: false, partialBytes: 0, sizeBytes: 1000 };

describe('downloadView', () => {
  it('shows live progress while the backend downloads, even when a .part exists', () => {
    // The bug: a remounted view saw the .part and offered "Resume" while the download still ran.
    const v = downloadView(live({ downloaded: 420 }), { ...file, partialBytes: 300 });
    expect(v).toMatchObject({ kind: 'active', phase: 'downloading', pct: 42, downloaded: 420 });
  });

  it('maps verifying / installing / starting to active states', () => {
    expect(downloadView(live({ phase: 'verifying', downloaded: 1000 }), file)).toMatchObject({ kind: 'active', phase: 'verifying', pct: 100 });
    expect(downloadView(live({ phase: 'installing' }), file).kind).toBe('active');
    expect(downloadView(live({ phase: 'starting', total: null }), file)).toMatchObject({ kind: 'active', pct: 0 });
  });

  it('offers Resume for a partial file and after a pause', () => {
    expect(downloadView(undefined, { ...file, partialBytes: 250 })).toEqual({ kind: 'resume', pct: 25, error: undefined });
    // Paused just now: the catalog's partial size may be stale; the backend count wins.
    expect(downloadView(live({ phase: 'paused', downloaded: 600 }), { ...file, partialBytes: 250 })).toMatchObject({ kind: 'resume', pct: 60 });
  });

  it('is ready when the file is complete, download otherwise', () => {
    expect(downloadView(live({ phase: 'done', downloaded: 1000 }), { ...file, downloaded: true })).toEqual({ kind: 'ready' });
    expect(downloadView(undefined, file)).toEqual({ kind: 'download', error: undefined });
  });

  it('surfaces the last error with a retry', () => {
    expect(downloadView(live({ phase: 'error', message: 'checksum mismatch' }), file)).toEqual({ kind: 'download', error: 'checksum mismatch' });
    expect(downloadView(live({ phase: 'error', message: 'connection lost', downloaded: 500 }), { ...file, partialBytes: 500 })).toMatchObject({ kind: 'resume', pct: 50, error: 'connection lost' });
  });

  it('classifies phases', () => {
    expect(['starting', 'downloading', 'verifying', 'installing'].every((p) => isActive(p as DownloadState['phase']))).toBe(true);
    expect(['done', 'paused', 'error'].some((p) => isActive(p as DownloadState['phase']))).toBe(false);
  });
});
