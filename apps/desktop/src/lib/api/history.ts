import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';

/**
 * Local history on this device (src-tauri/src/modules/history.rs): file versions (local file
 * history), utility runs, the history settings and disk usage. Nothing here is ever written to the
 * project — it lives in the app data folder.
 */

/** Who made a file version. */
export type VersionSource = 'initial' | 'external' | 'app' | 'agent' | 'restore';

export interface FileVersion {
  id: number;
  /** Project-relative, `/`-separated (`http/a.req.yaml`, `devall.yaml`). */
  path: string;
  at: number;
  size: number;
  source: VersionSource;
  /** The file was deleted (no content). */
  deleted: boolean;
}

export interface FileSummary {
  path: string;
  versions: number;
  lastAt: number;
  lastSource: VersionSource;
  deleted: boolean;
}

export interface UtilityRun {
  id: string;
  at: number;
  op: string;
  opName: string;
  input: string;
  inputTruncated: boolean;
  /** The input was a secret (TOTP seed, password) and isn't stored. */
  inputMasked: boolean;
  /** Secret values are `UTIL_MASK`. */
  args: Record<string, string>;
  output: string;
  outputTruncated: boolean;
  ok: boolean;
  error: string | null;
  project: string | null;
}

export interface NewUtilityRun {
  op: string;
  opName: string;
  input: string;
  args: Record<string, string>;
  /** Names of arguments of kind `secret` (always masked). */
  secretArgs: string[];
  output: string;
  error?: string | null;
  project?: string | null;
}

export interface HistorySettings {
  responseBodies: boolean;
  responsesPerRequest: number;
  utilities: boolean;
  utilitiesMax: number;
  fileVersions: boolean;
  fileMaxVersions: number;
  fileMaxDays: number;
  fileMaxMb: number;
}

export interface KindUsage {
  count: number | null;
  bytes: number;
}

export interface HistoryUsage {
  dataDir: string;
  projectDir: string | null;
  responses: KindUsage;
  files: KindUsage;
  utilities: KindUsage;
  events: KindUsage;
  webhooks: KindUsage;
  loadRuns: KindUsage;
  caches: KindUsage;
}

export type ClearableKind = 'responses' | 'files' | 'utilities' | 'events';

/** A past AI "Ask" question and answer (last 50 per project, on this device). */
export interface AskRecord {
  id: string;
  at: number;
  question: string;
  answer: string;
  citations: { id: string; kind: string; source: string; title: string }[];
  /** `provider / model` that answered. */
  model?: string;
}

/** What the backend stores instead of a secret argument value. */
export const UTIL_MASK = '••••••';

export const historyApi = {
  fileVersions: (root: string, path: string, limit?: number) => invoke<FileVersion[]>('fh_versions', { root, path, limit }),
  recentVersions: (root: string, query?: string, offset?: number, limit?: number) => invoke<FileVersion[]>('fh_recent', { root, query: query || undefined, offset, limit }),
  files: (root: string, query?: string) => invoke<FileSummary[]>('fh_files', { root, query: query || undefined }),
  content: (root: string, id: number) => invoke<string | null>('fh_content', { root, id }),
  /** A YAML version parsed to JSON (`null` for a deletion or invalid YAML). */
  yaml: <T = unknown>(root: string, id: number) => invoke<T | null>('fh_yaml', { root, id }),
  restore: (root: string, id: number) => invoke<string>('fh_restore', { root, id }),
  deleteVersion: (root: string, id: number) => invoke<boolean>('fh_delete', { root, id }),
  clearFile: (root: string, path?: string) => invoke<number>('fh_clear', { root, path }),

  recordUtility: (run: NewUtilityRun) => invoke<string | null>('uh_record', { run }),
  utilityRuns: (op?: string, query?: string, offset?: number, limit?: number) => invoke<UtilityRun[]>('uh_list', { op, query: query || undefined, offset, limit }),
  deleteUtilityRun: (id: string) => invoke<boolean>('uh_delete', { id }),
  clearUtilityRuns: (op?: string) => invoke<number>('uh_clear', { op }),

  askHistory: (root: string) => invoke<AskRecord[]>('ai_ask_history', { root }),
  addAsk: (root: string, entry: AskRecord) => invoke<void>('ai_ask_history_add', { root, entry }),
  clearAsks: (root: string) => invoke<void>('ai_ask_history_clear', { root }),

  settings: () => invoke<HistorySettings>('hist_settings'),
  setSettings: (settings: HistorySettings) => invoke<HistorySettings>('hist_set_settings', { settings }),
  usage: (root?: string | null) => invoke<HistoryUsage>('hist_usage', { root: root ?? undefined }),
  clear: (kind: ClearableKind, root?: string | null) => invoke<number>('hist_clear', { kind, root: root ?? undefined }),
  dataDir: (root?: string | null) => invoke<string>('hist_data_dir', { root: root ?? undefined }),
};

/** Arguments of a recorded utility run to restore: masked secrets are left out (the tool keeps its value). */
export function restorableArgs(args: Record<string, string>): Record<string, string> {
  return Object.fromEntries(Object.entries(args).filter(([, v]) => v !== UTIL_MASK));
}

/** Labels for version sources, as shown in lists. */
export const SOURCE_LABEL: Record<VersionSource, string> = {
  initial: 'First seen',
  external: 'Changed outside (editor, git…)',
  app: 'Saved in /dev/all',
  agent: 'Edited by an agent',
  restore: 'Restored',
};

export function versionLabel(v: Pick<FileVersion, 'source' | 'deleted'>): string {
  return v.deleted ? 'Deleted' : SOURCE_LABEL[v.source];
}

/**
 * Bumped when local file history changes (the backend's `file-history-changed` event: a scan, a
 * watcher batch, a restore) and when utility runs are recorded, so open history lists refresh.
 */
export const useLocalHistoryVersion = create<{ files: number; utilities: number; bumpFiles(): void; bumpUtilities(): void }>((set) => ({
  files: 0,
  utilities: 0,
  bumpFiles: () => set((s) => ({ files: s.files + 1 })),
  bumpUtilities: () => set((s) => ({ utilities: s.utilities + 1 })),
}));

let listening = false;

/** Subscribes once (app start) to the backend's file-history events. */
export function initLocalHistoryEvents() {
  if (listening) return;
  listening = true;
  void listen<{ root: string }>('file-history-changed', () => useLocalHistoryVersion.getState().bumpFiles()).catch(() => {
    listening = false;
  });
}
