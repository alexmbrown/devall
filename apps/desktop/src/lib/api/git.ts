import { invoke } from '@tauri-apps/api/core';

export interface GitFile {
  path: string;
  staged: string;
  unstaged: string;
  from: string | null;
}

export interface GitStatus {
  isRepo: boolean;
  repoRoot: string | null;
  projectPrefix: string;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  files: GitFile[];
  hasScanHook: boolean;
}

export interface GitCommit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

export const gitApi = {
  status: (root: string) => invoke<GitStatus>('git_status', { root }),
  diff: (root: string, path: string, staged: boolean) => invoke<{ original: string; modified: string; binary: boolean }>('git_diff', { root, path, staged }),
  stage: (root: string, paths: string[]) => invoke<void>('git_stage', { root, paths }),
  unstage: (root: string, paths: string[]) => invoke<void>('git_unstage', { root, paths }),
  discard: (root: string, paths: string[]) => invoke<void>('git_discard', { root, paths }),
  commit: (root: string, message: string, amend = false) => invoke<string>('git_commit', { root, message, amend }),
  log: (root: string, path?: string, limit = 50) => invoke<GitCommit[]>('git_log', { root, path, limit }),
  sync: (root: string, action: 'fetch' | 'pull' | 'push') => invoke<string>('git_sync', { root, action }),
  branches: (root: string) => invoke<string[]>('git_branches', { root }),
  checkout: (root: string, branch: string, create = false) => invoke<void>('git_checkout', { root, branch, create }),
  init: (root: string) => invoke<void>('git_init', { root }),
  installScanHook: (root: string) => invoke<string>('git_install_scan_hook', { root }),
};
