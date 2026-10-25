import { Channel, invoke } from '@tauri-apps/api/core';

// Mirrors of devall_core::repos (Repo, git::Summary, …) and src-tauri/src/modules/git.rs. Every
// command names the repository it acts on by name (`repo`; omitted = the project's default repo):
// the backend resolves the folder from the project's repo list, never from a path we send.

export interface GitFile {
  path: string;
  staged: string;
  unstaged: string;
  from: string | null;
}

export interface GitStatus {
  isRepo: boolean;
  repo: string | null;
  repoRoot: string | null;
  /** Project folder relative to the repo root ('' = same; null = the project isn't inside it). */
  projectPrefix: string | null;
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

export type RepoState = 'found' | 'missing' | 'notARepo';
/** `manifest` = listed in devall.yaml; `containing` = the repo the project folder is in; `discovered` = nested, not listed. */
export type RepoOrigin = 'manifest' | 'containing' | 'discovered';

export interface Repo {
  name: string;
  /** Absolute folder on this machine. */
  path: string;
  origin: RepoOrigin;
  state: RepoState;
  /** As written in devall.yaml (listed) or relative to the project (discovered). */
  relPath: string | null;
  remote: string | null;
  group: string | null;
  /** The location comes from this machine's repos.json. */
  overridden: boolean;
  projectPrefix: string | null;
}

export interface RepoSummary {
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  changes: number;
  staged: number;
  conflicts: number;
  lastCommit: GitCommit | null;
  hasScanHook: boolean;
  error: string | null;
}

export interface RepoOverview {
  repo: Repo;
  summary: RepoSummary | null;
}

export interface RepoRef {
  name: string;
  path: string;
  remote?: string | null;
  group?: string | null;
}

export interface RepoSuggestion {
  name: string;
  relPath: string | null;
  farAway: boolean;
  isGit: boolean;
  remote: string | null;
  existing: string | null;
}

export interface ComposeFile {
  repo: string;
  group: string | null;
  /** Absolute. */
  path: string;
  /** Relative to the repository. */
  relPath: string;
  /** Default Compose project name (the folder's name). */
  project: string;
}

export interface BulkResult {
  repo: string;
  ok: boolean;
  message: string;
}

export interface RepoBranches {
  repo: string;
  branches: { current: string | null; local: string[]; remote: string[] } | null;
  error: string | null;
}

/** Prefix of repo-qualified paths in project files: `repo:<name>/<path>`. */
export const REPO_LINK = 'repo:';

/** `repo:web/src/app.ts` → `{ repo: 'web', path: 'src/app.ts' }`. */
export function parseRepoLink(s: string): { repo: string; path: string } | null {
  const m = /^repo:[\\/]*([^\\/]+)(?:[\\/](.*))?$/.exec(s.trim());
  return m ? { repo: m[1], path: (m[2] ?? '').replace(/\\/g, '/') } : null;
}

export const gitApi = {
  status: (root: string, repo?: string | null) => invoke<GitStatus>('git_status', { root, repo }),
  diff: (root: string, repo: string | null | undefined, path: string, staged: boolean) => invoke<{ original: string; modified: string; binary: boolean }>('git_diff', { root, repo, path, staged }),
  stage: (root: string, repo: string | null | undefined, paths: string[]) => invoke<void>('git_stage', { root, repo, paths }),
  unstage: (root: string, repo: string | null | undefined, paths: string[]) => invoke<void>('git_unstage', { root, repo, paths }),
  discard: (root: string, repo: string | null | undefined, paths: string[]) => invoke<void>('git_discard', { root, repo, paths }),
  commit: (root: string, repo: string | null | undefined, message: string, amend = false) => invoke<string>('git_commit', { root, repo, message, amend }),
  log: (root: string, repo?: string | null, path?: string, limit = 50) => invoke<GitCommit[]>('git_log', { root, repo, path, limit }),
  sync: (root: string, repo: string | null | undefined, action: 'fetch' | 'pull' | 'push') => invoke<string>('git_sync', { root, repo, action }),
  branches: (root: string, repo?: string | null) => invoke<string[]>('git_branches', { root, repo }),
  checkout: (root: string, repo: string | null | undefined, branch: string, create = false) => invoke<void>('git_checkout', { root, repo, branch, create }),
  init: (root: string, repo?: string | null) => invoke<void>('git_init', { root, repo }),
  installScanHook: (root: string, repo?: string | null) => invoke<string>('git_install_scan_hook', { root, repo }),

  repos: (root: string) => invoke<Repo[]>('git_repos', { root }),
  /** Branch, upstream, changes and last commit of every repo (or only `repos`), run concurrently. */
  overview: (root: string, repos?: string[]) => invoke<RepoOverview[]>('git_overview', { root, repos }),
  /** Fetch all / Pull all over the named repos; per-repo results. */
  bulk: (root: string, repos: string[], action: 'fetch' | 'pull') => invoke<BulkResult[]>('git_bulk', { root, repos, action }),
  branchesAll: (root: string, repos: string[]) => invoke<RepoBranches[]>('git_branches_all', { root, repos }),
  switchAll: (root: string, repos: string[], branch: string, create: boolean) => invoke<BulkResult[]>('git_switch_all', { root, repos, branch, create }),
  watch: (root: string) => invoke<void>('git_watch', { root }),
  unwatch: () => invoke<void>('git_unwatch'),
};

export const reposApi = {
  suggest: (root: string, folder: string) => invoke<RepoSuggestion>('repos_suggest', { root, folder }),
  add: (root: string, folder: string, opts: { name?: string; group?: string; localOnly?: boolean } = {}) =>
    invoke<RepoRef>('repos_add', { root, folder, name: opts.name, group: opts.group, localOnly: !!opts.localOnly }),
  update: (root: string, name: string, changes: { newName?: string; group?: string }) => invoke<void>('repos_update', { root, name, newName: changes.newName, group: changes.group }),
  remove: (root: string, name: string) => invoke<void>('repos_remove', { root, name }),
  locate: (root: string, name: string, folder: string) => invoke<void>('repos_locate', { root, name, folder }),
  saveDiscovered: (root: string, names: string[], groups?: Record<string, string>) => invoke<RepoRef[]>('repos_save_discovered', { root, names, groups }),
  /** Is the folder a project already, and which repositories are in it (containing one first, then nested). */
  detect: (folder: string) => invoke<{ isProject: boolean; repos: Repo[] }>('repos_detect', { folder }),
  clone: (root: string, name: string, onProgress: (line: string) => void) => {
    const ch = new Channel<string>();
    ch.onmessage = onProgress;
    return invoke<Repo>('repos_clone', { root, name, onProgress: ch });
  },
  /** Absolute path of a `repo:<name>/<path>` link. */
  resolve: (root: string, link: string) => invoke<string>('repos_resolve', { root, link }),
  /** Docker Compose files in the project's repositories. */
  composeFiles: (root: string) => invoke<ComposeFile[]>('repos_compose_files', { root }),
  /** How to store a picked file: `repo:<name>/…`, project-relative or absolute. */
  portable: (root: string, path: string) => invoke<string>('repos_portable', { root, path }),
};
