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

/** A multi-step operation git has paused (usually on conflicts). */
export type GitOperation = 'merge' | 'rebase' | 'cherryPick' | 'revert';

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
  operation?: GitOperation | null;
  /** `index.lock` exists: another git is running, or one crashed and left it. */
  indexLock?: { path: string; ageSecs: number } | null;
}

// ---------- compare & review (devall_core::repos::compare / review) ----------

/** `mergeBase`: changes since the head branched off (what an MR shows); `direct`: tip to tip. */
export type CompareMode = 'mergeBase' | 'direct';
export type RefKind = 'branch' | 'remote' | 'tag' | 'commit';

export interface RefInfo {
  name: string;
  sha: string;
  short: string;
  kind: RefKind;
  refname: string | null;
}

export interface ChangedFile {
  path: string;
  oldPath: string | null;
  /** A, M, D, R, C, T or U. */
  status: string;
  similarity: number | null;
  /** null for binary files. */
  added: number | null;
  deleted: number | null;
  binary: boolean;
  oldBlob: string | null;
  newBlob: string | null;
  generated: boolean;
  untracked: boolean;
}

export interface RangeCommit {
  hash: string;
  short: string;
  parents: string[];
  author: string;
  email: string;
  date: string;
  subject: string;
  /** The same change is already on the base (cherry-picked / rebased). */
  equivalent: boolean;
}

export interface Comparison {
  base: RefInfo;
  head: RefInfo;
  mode: CompareMode;
  mergeBase: string | null;
  mergeBases: number;
  unrelated: boolean;
  from: string;
  /** null = the working tree. */
  to: string | null;
  headIsCurrent: boolean;
  uncommitted: boolean;
  ahead: number;
  behind: number;
  files: ChangedFile[];
  commits: RangeCommit[];
  commitsTruncated: boolean;
  baseRemote: string | null;
  fetchedSecsAgo: number | null;
  remoteUrl: string | null;
  baseRemoteBranch: string | null;
  headRemoteBranch: string | null;
  /** The head is (or tracks) a branch on the remote. */
  headPushed: boolean;
}

export interface ConflictPrediction {
  supported: boolean;
  clean: boolean;
  files: string[];
  message: string | null;
  gitVersion: string;
}

export interface CompareDefaults {
  base: string | null;
  current: string | null;
  head: string | null;
  gitVersion: string;
  canPredictConflicts: boolean;
}

export interface CompareHint {
  base: string;
  head: string;
  ahead: number;
  behind: number;
}

export interface RangeFiles {
  from: string;
  to: string;
  files: ChangedFile[];
}

/** One side of a file diff (paths are inside the repository). */
export type DiffSide = { type: 'rev'; rev: string; path: string } | { type: 'blob'; id: string } | { type: 'worktree'; path: string } | { type: 'index'; path: string } | { type: 'empty' };

export interface FileDiff {
  original: string;
  modified: string;
  binary: boolean;
  /** Data URLs for image files. */
  image: { old: string | null; new: string | null } | null;
  tooLarge: boolean;
  oldSize: number;
  newSize: number;
  oldMissing: boolean;
  newMissing: boolean;
}

export interface CommitInfo {
  hash: string;
  short: string;
  parents: string[];
  author: string;
  email: string;
  date: string;
  committer: string;
  committerEmail: string;
  committerDate: string;
  subject: string;
  body: string;
  refs: string[];
}

export interface Reviewed {
  blob: string;
  /** Unix seconds. */
  at: number;
}

export interface RefEntry {
  name: string;
  refname: string;
  sha: string;
  kind: RefKind;
  upstream: string | null;
  track: string | null;
  time: number;
  subject: string;
  current: boolean;
}

export interface RefList {
  current: string | null;
  head: string | null;
  detached: boolean;
  refs: RefEntry[];
}

// ---------- commit graph (devall_core::repos::graph) ----------

/** `[kind, from, to, colour]`: 0 = lane passing by, 1 = lane ending in the node, 2 = parent line leaving the node. */
export type GraphSeg = [number, number, number, number];

export interface RefLabel {
  name: string;
  kind: 'head' | 'branch' | 'remote' | 'tag' | 'other';
  current: boolean;
}

export interface GraphRow {
  /** `WIP` for the uncommitted-changes row. */
  hash: string;
  parents: string[];
  author: string;
  email: string;
  /** Unix seconds. */
  time: number;
  subject: string;
  refs: RefLabel[];
  col: number;
  color: number;
  segs: GraphSeg[];
  width: number;
}

export interface GraphPage {
  offset: number;
  rows: GraphRow[];
  loaded: number;
  done: boolean;
  error: string | null;
  maxWidth: number;
  elapsedMs: number;
}

// ---------- hunk / line staging (devall_core::repos::patch) ----------

export interface PatchLine {
  kind: ' ' | '+' | '-';
  text: string;
  old: number | null;
  new: number | null;
  noEol: boolean;
}

export interface PatchHunk {
  header: string;
  lines: PatchLine[];
}

export interface PatchView {
  path: string;
  staged: boolean;
  binary: boolean;
  fingerprint: string;
  hunks: PatchHunk[];
  lineCount: number;
  tooLarge: boolean;
  crlf: boolean;
}

export type LineAction = 'stage' | 'unstage' | 'discard';

/** Lines (indices within the hunk) to take from one hunk; null = the whole hunk. */
export interface LineSelection {
  hunk: number;
  lines: number[] | null;
}

export interface OpResult {
  conflicts: boolean;
  message: string;
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

  // compare & review
  compareDefaults: (root: string, repo: string | null | undefined) => invoke<CompareDefaults>('git_compare_defaults', { root, repo }),
  refs: (root: string, repo: string | null | undefined) => invoke<RefList>('git_refs', { root, repo }),
  compare: (root: string, repo: string | null | undefined, base: string, head: string, mode: CompareMode, uncommitted: boolean) => invoke<Comparison>('git_compare', { root, repo, base, head, mode, uncommitted }),
  compareConflicts: (root: string, repo: string | null | undefined, base: string, head: string) => invoke<ConflictPrediction>('git_compare_conflicts', { root, repo, base, head }),
  compareHint: (root: string, repo: string | null | undefined, base?: string | null) => invoke<CompareHint | null>('git_compare_hint', { root, repo, base }),
  commitRange: (root: string, repo: string | null | undefined, oldest: string, newest: string) => invoke<RangeFiles>('git_commit_range', { root, repo, oldest, newest }),
  fileDiff: (root: string, repo: string | null | undefined, old: DiffSide, newSide: DiffSide, path: string, force = false) => invoke<FileDiff>('git_file_diff', { root, repo, old, new: newSide, path, force }),
  commitInfo: (root: string, repo: string | null | undefined, rev: string) => invoke<CommitInfo>('git_commit_info', { root, repo, rev }),
  reviewGet: (root: string, repo: string | null | undefined, base: string, head: string) => invoke<Record<string, Reviewed>>('git_review_get', { root, repo, base, head }),
  reviewSet: (root: string, repo: string | null | undefined, base: string, head: string, changes: { path: string; blob: string | null }[]) => invoke<Record<string, Reviewed>>('git_review_set', { root, repo, base, head, changes }),

  // commit graph
  graphOpen: (root: string, repo: string | null | undefined, wip: boolean, refresh = false) => invoke<{ generation: number; reused: boolean }>('git_graph_open', { root, repo, wip, refresh }),
  graphRows: (root: string, repo: string | null | undefined, generation: number, offset: number, limit: number) => invoke<GraphPage>('git_graph_rows', { root, repo, generation, offset, limit }),
  graphSearch: (root: string, repo: string | null | undefined, generation: number, query: string) => invoke<number[]>('git_graph_search', { root, repo, generation, query }),
  graphFind: (root: string, repo: string | null | undefined, generation: number, hash: string) => invoke<number | null>('git_graph_find', { root, repo, generation, hash }),

  // hunk / line staging
  patch: (root: string, repo: string | null | undefined, path: string, staged: boolean, ignoreWs = false, force = false) => invoke<PatchView>('git_patch', { root, repo, path, staged, ignoreWs, force }),
  applyLines: (root: string, repo: string | null | undefined, path: string, action: LineAction, fingerprint: string, selection: LineSelection[], ignoreWs = false) =>
    invoke<void>('git_apply_lines', { root, repo, path, action, fingerprint, selection, ignoreWs }),

  // commit & ref operations
  branchCreate: (root: string, repo: string | null | undefined, name: string, at: string, checkout: boolean) => invoke<void>('git_branch_create', { root, repo, name, at, checkout }),
  tagCreate: (root: string, repo: string | null | undefined, name: string, at: string, message?: string | null) => invoke<void>('git_tag_create', { root, repo, name, at, message }),
  checkoutCommit: (root: string, repo: string | null | undefined, rev: string) => invoke<void>('git_checkout_commit', { root, repo, rev }),
  checkoutRef: (root: string, repo: string | null | undefined, name: string, kind: 'branch' | 'remote') => invoke<void>('git_checkout_ref', { root, repo, name, kind }),
  cherryPick: (root: string, repo: string | null | undefined, rev: string) => invoke<OpResult>('git_cherry_pick', { root, repo, rev }),
  revert: (root: string, repo: string | null | undefined, rev: string) => invoke<OpResult>('git_revert', { root, repo, rev }),
  operationAbort: (root: string, repo: string | null | undefined) => invoke<void>('git_operation_abort', { root, repo }),
  operationContinue: (root: string, repo: string | null | undefined) => invoke<string>('git_operation_continue', { root, repo }),
  clearLock: (root: string, repo: string | null | undefined) => invoke<boolean>('git_clear_lock', { root, repo }),
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
