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
  ahead: number;
  behind: number;
  /** The upstream branch was deleted on the remote. */
  gone: boolean;
  time: number;
  subject: string;
  current: boolean;
  /** Checked out in another worktree (its path). */
  worktree: string | null;
  annotated: boolean;
}

export interface RefList {
  current: string | null;
  head: string | null;
  detached: boolean;
  refs: RefEntry[];
  remotes: string[];
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
  /** Stopped with conflicts: resolve them, then continue or abort. */
  conflicts: boolean;
  /** Stopped without conflicts (an interactive rebase's `edit`). */
  paused: boolean;
  message: string;
}

export type MergeMode = 'ff' | 'noFf' | 'ffOnly' | 'squash';
export type ResetMode = 'soft' | 'mixed' | 'keep' | 'hard';
export type PushMode = 'normal' | 'setUpstream' | 'forceWithLease';

export interface PushInfo {
  branch: string | null;
  upstream: string | null;
  remote: string | null;
  remoteBranch: string | null;
  ahead: number;
  behind: number;
  remotes: string[];
  forceIfIncludes: boolean;
}

export interface RemoteInfo {
  name: string;
  /** Credentials stripped. */
  fetchUrl: string | null;
  pushUrl: string | null;
  branches: number;
}

export interface StashEntry {
  index: number;
  name: string;
  oid: string;
  message: string;
  title: string;
  branch: string | null;
  time: number;
  base: string;
  untracked: boolean;
}

export interface StashSaveOptions {
  message?: string | null;
  includeUntracked?: boolean;
  stagedOnly?: boolean;
  keepIndex?: boolean;
}

export interface StashFiles {
  base: string;
  commit: string;
  files: ChangedFile[];
  untrackedCommit: string | null;
}

// ---------- undo / redo (devall_core::repos::undo) ----------

export type UndoKind =
  | 'commit'
  | 'amend'
  | 'stage'
  | 'unstage'
  | 'discard'
  | 'checkout'
  | 'branchCreate'
  | 'branchDelete'
  | 'branchRename'
  | 'setUpstream'
  | 'fastForward'
  | 'reset'
  | 'merge'
  | 'rebase'
  | 'cherryPick'
  | 'revert'
  | 'pull'
  | 'stashSave'
  | 'stashApply'
  | 'stashPop'
  | 'stashDrop'
  | 'tagCreate'
  | 'tagDelete';

export interface UndoEntry {
  id: string;
  kind: UndoKind;
  label: string;
  /** Unix seconds. */
  at: number;
  /** The operation is paused (conflicts); completed when it finishes. */
  pending: boolean;
}

export interface UndoHistory {
  /** Newest first: the next undo is `undo[0]`. */
  undo: UndoEntry[];
  /** The next redo first. */
  redo: UndoEntry[];
}

export type UndoOutcome = { type: 'done'; label: string; kind: UndoKind } | { type: 'confirm'; label: string; reasons: string[] } | { type: 'refused'; label: string; reason: string } | { type: 'nothing' };

// ---------- merge conflicts (devall_core::repos::conflicts) ----------

export interface ConflictFile {
  path: string;
  base: boolean;
  ours: boolean;
  theirs: boolean;
  description: string;
}

export interface ConflictVersion {
  text: string | null;
  exists: boolean;
  binary: boolean;
  tooLarge: boolean;
  size: number;
}

export interface Conflict {
  path: string;
  description: string;
  base: ConflictVersion;
  ours: ConflictVersion;
  theirs: ConflictVersion;
  /** The working-tree file with git's markers. */
  result: ConflictVersion;
  binary: boolean;
  labels: { ours: string; theirs: string; base: string };
  operation: GitOperation | null;
  resolved: boolean;
}

export type ConflictPick = 'ours' | 'theirs';

// ---------- interactive rebase (devall_core::repos::rebase) ----------

export interface TodoCommit {
  hash: string;
  short: string;
  subject: string;
  body: string;
  author: string;
  date: string;
  /** Other local branches pointing at it (moved along with update-refs). */
  branches: string[];
  merge: boolean;
}

export interface RebaseTodo {
  base: string;
  baseShort: string;
  branch: string | null;
  /** Oldest first. */
  commits: TodoCommit[];
  hasMerges: boolean;
  canUpdateRefs: boolean;
}

export type RebaseAction = 'pick' | 'reword' | 'edit' | 'squash' | 'fixup' | 'drop';

export interface RebaseStep {
  action: RebaseAction;
  hash: string;
  message?: string | null;
}

// ---------- blame & file history (devall_core::repos::blame) ----------

export interface BlameCommit {
  hash: string;
  short: string;
  author: string;
  email: string;
  /** Unix seconds. */
  time: number;
  summary: string;
  previous: [string, string] | null;
  filename: string;
  uncommitted: boolean;
  boundary: boolean;
}

export interface BlameLine {
  commit: number;
  origLine: number;
  text: string;
}

export interface Blame {
  path: string;
  rev: string | null;
  commits: BlameCommit[];
  lines: BlameLine[];
  ignoredRevs: boolean;
}

export interface FileVersion {
  hash: string;
  short: string;
  parent: string | null;
  author: string;
  email: string;
  date: string;
  subject: string;
  path: string;
  oldPath: string;
  status: string;
}

// ---------- worktrees (devall_core::repos::worktree) ----------

export interface Worktree {
  path: string;
  head: string | null;
  branch: string | null;
  detached: boolean;
  bare: boolean;
  locked: string | null;
  prunable: string | null;
  main: boolean;
  current: boolean;
}

export type WorktreeCheckout = { type: 'newBranch'; name: string; from: string } | { type: 'branch'; name: string } | { type: 'detached'; rev: string };

export interface StagedForAi {
  diff: string;
  branch: string | null;
  recent: string[];
  truncated: boolean;
}

// ---------- pull / merge requests (devall-forge) ----------

export type ForgeKind = 'github' | 'gitlab';

export interface ForgeRemote {
  kind: ForgeKind;
  host: string;
  path: string;
  webUrl: string;
  apiUrl: string;
}

export interface ForgeAccount {
  host: string;
  kind: ForgeKind;
  user: string | null;
  apiUrl?: string | null;
}

export interface PrTemplate {
  name: string;
  path: string;
  content: string;
}

export interface ForgeInfo {
  remoteName: string | null;
  remoteUrl: string | null;
  forge: ForgeRemote | null;
  connected: boolean;
  account: ForgeAccount | null;
  tokenHelp: { url: string; scopes: string } | null;
  cli: { program: string; installed: boolean } | null;
  templates: PrTemplate[];
  remotes: string[];
  keychain: boolean;
}

export interface NewPullRequest {
  title: string;
  body: string;
  base: string;
  head: string;
  draft: boolean;
  reviewers: string[];
  labels: string[];
}

export interface CreatedPullRequest {
  number: number;
  url: string;
  warnings: string[];
  viaCli: boolean;
}

export interface GitCommit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

export type RepoState = 'found' | 'missing' | 'notARepo';
/** `manifest` = listed in devall.yaml; `containing` = the repo the project folder is in; `discovered` = nested, not listed; `local` = opened on this machine only (a worktree), never written to devall.yaml. */
export type RepoOrigin = 'manifest' | 'containing' | 'discovered' | 'local';

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
  operationContinue: (root: string, repo: string | null | undefined) => invoke<OpResult>('git_operation_continue', { root, repo }),
  operationSkip: (root: string, repo: string | null | undefined) => invoke<OpResult>('git_operation_skip', { root, repo }),

  // branches, tags, history operations
  branchRename: (root: string, repo: string | null | undefined, name: string, newName: string) => invoke<void>('git_branch_rename', { root, repo, name, newName }),
  branchDelete: (root: string, repo: string | null | undefined, name: string, force: boolean) => invoke<void>('git_branch_delete', { root, repo, name, force }),
  branchUnmerged: (root: string, repo: string | null | undefined, name: string) => invoke<number>('git_branch_unmerged', { root, repo, name }),
  branchDeleteRemote: (root: string, repo: string | null | undefined, remoteBranch: string) => invoke<string>('git_branch_delete_remote', { root, repo, remoteBranch }),
  branchUpstream: (root: string, repo: string | null | undefined, branch: string, upstream: string | null) => invoke<void>('git_branch_upstream', { root, repo, branch, upstream }),
  branchFastForward: (root: string, repo: string | null | undefined, branch: string, from?: string | null) => invoke<string>('git_branch_ff', { root, repo, branch, from }),
  branchMove: (root: string, repo: string | null | undefined, branch: string, rev: string) => invoke<void>('git_branch_move', { root, repo, branch, rev }),
  merge: (root: string, repo: string | null | undefined, rev: string, mode: MergeMode) => invoke<OpResult>('git_merge', { root, repo, rev, mode }),
  rebase: (root: string, repo: string | null | undefined, onto: string) => invoke<OpResult>('git_rebase', { root, repo, onto }),
  reset: (root: string, repo: string | null | undefined, rev: string, mode: ResetMode) => invoke<void>('git_reset', { root, repo, rev, mode }),
  tagDelete: (root: string, repo: string | null | undefined, name: string) => invoke<void>('git_tag_delete', { root, repo, name }),
  tagPush: (root: string, repo: string | null | undefined, remote: string, name: string) => invoke<string>('git_tag_push', { root, repo, remote, name }),
  tagDeleteRemote: (root: string, repo: string | null | undefined, remote: string, name: string) => invoke<string>('git_tag_delete_remote', { root, repo, remote, name }),

  // remotes
  remotes: (root: string, repo: string | null | undefined) => invoke<RemoteInfo[]>('git_remotes', { root, repo }),
  remoteAdd: (root: string, repo: string | null | undefined, name: string, url: string) => invoke<void>('git_remote_add', { root, repo, name, url }),
  remoteRename: (root: string, repo: string | null | undefined, name: string, newName: string) => invoke<void>('git_remote_rename', { root, repo, name, newName }),
  remoteSetUrl: (root: string, repo: string | null | undefined, name: string, url: string) => invoke<void>('git_remote_set_url', { root, repo, name, url }),
  remoteRemove: (root: string, repo: string | null | undefined, name: string) => invoke<void>('git_remote_remove', { root, repo, name }),
  remotePrune: (root: string, repo: string | null | undefined, name: string) => invoke<string>('git_remote_prune', { root, repo, name }),
  fetch: (root: string, repo: string | null | undefined, remote: string | null, prune: boolean) => invoke<string>('git_fetch', { root, repo, remote, prune }),

  // stash
  stashList: (root: string, repo: string | null | undefined) => invoke<StashEntry[]>('git_stash_list', { root, repo }),
  stashSave: (root: string, repo: string | null | undefined, options: StashSaveOptions) => invoke<string>('git_stash_save', { root, repo, options }),
  stashApply: (root: string, repo: string | null | undefined, index: number, oid: string, pop: boolean) => invoke<OpResult>('git_stash_apply', { root, repo, index, oid, pop }),
  stashDrop: (root: string, repo: string | null | undefined, index: number, oid: string) => invoke<void>('git_stash_drop', { root, repo, index, oid }),
  stashFiles: (root: string, repo: string | null | undefined, index: number, oid: string) => invoke<StashFiles>('git_stash_files', { root, repo, index, oid }),

  // push
  pushInfo: (root: string, repo: string | null | undefined) => invoke<PushInfo>('git_push_info', { root, repo }),
  push: (root: string, repo: string | null | undefined, mode: PushMode, remote?: string | null, branch?: string | null) => invoke<string>('git_push', { root, repo, mode, remote, branch }),

  // undo / redo
  undoHistory: (root: string, repo: string | null | undefined) => invoke<UndoHistory>('git_undo_history', { root, repo }),
  undo: (root: string, repo: string | null | undefined, force = false) => invoke<UndoOutcome>('git_undo', { root, repo, force }),
  redo: (root: string, repo: string | null | undefined, force = false) => invoke<UndoOutcome>('git_redo', { root, repo, force }),
  undoClear: (root: string, repo: string | null | undefined) => invoke<void>('git_undo_clear', { root, repo }),

  // merge conflicts
  conflicts: (root: string, repo: string | null | undefined) => invoke<ConflictFile[]>('git_conflicts', { root, repo }),
  conflictRead: (root: string, repo: string | null | undefined, path: string) => invoke<Conflict>('git_conflict_read', { root, repo, path }),
  conflictSave: (root: string, repo: string | null | undefined, path: string, content: string, markResolved: boolean) => invoke<void>('git_conflict_save', { root, repo, path, content, markResolved }),
  conflictTake: (root: string, repo: string | null | undefined, path: string, side: ConflictPick) => invoke<void>('git_conflict_take', { root, repo, path, side }),
  conflictRemove: (root: string, repo: string | null | undefined, path: string) => invoke<void>('git_conflict_remove', { root, repo, path }),
  conflictRecreate: (root: string, repo: string | null | undefined, path: string) => invoke<void>('git_conflict_recreate', { root, repo, path }),
  conflictMark: (root: string, repo: string | null | undefined, paths: string[]) => invoke<void>('git_conflict_mark', { root, repo, paths }),

  // interactive rebase
  rebaseTodo: (root: string, repo: string | null | undefined, base: string) => invoke<RebaseTodo>('git_rebase_todo', { root, repo, base }),
  rebaseAutosquash: (root: string, repo: string | null | undefined, base: string, steps: RebaseStep[]) => invoke<RebaseStep[]>('git_rebase_autosquash', { root, repo, base, steps }),
  rebaseInteractive: (root: string, repo: string | null | undefined, base: string, steps: RebaseStep[], updateRefs: boolean) => invoke<OpResult>('git_rebase_interactive', { root, repo, base, steps, updateRefs }),

  // blame & file history
  blame: (root: string, repo: string | null | undefined, path: string, rev?: string | null) => invoke<Blame>('git_blame', { root, repo, path, rev }),
  fileHistory: (root: string, repo: string | null | undefined, path: string, limit?: number) => invoke<FileVersion[]>('git_file_history', { root, repo, path, limit }),

  // worktrees
  worktrees: (root: string, repo: string | null | undefined) => invoke<Worktree[]>('git_worktrees', { root, repo }),
  worktreeSuggest: (root: string, repo: string | null | undefined, branch: string) => invoke<string>('git_worktree_suggest', { root, repo, branch }),
  worktreeAdd: (root: string, repo: string | null | undefined, path: string, checkout: WorktreeCheckout) => invoke<Worktree>('git_worktree_add', { root, repo, path, checkout }),
  worktreeLock: (root: string, repo: string | null | undefined, path: string, reason?: string | null) => invoke<void>('git_worktree_lock', { root, repo, path, reason }),
  worktreeUnlock: (root: string, repo: string | null | undefined, path: string) => invoke<void>('git_worktree_unlock', { root, repo, path }),
  worktreeRemove: (root: string, repo: string | null | undefined, path: string, force: boolean) => invoke<void>('git_worktree_remove', { root, repo, path, force }),
  worktreePrune: (root: string, repo: string | null | undefined) => invoke<string>('git_worktree_prune', { root, repo }),
  /** Shows a worktree in the repository list on this machine only; returns its name there. */
  worktreeOpen: (root: string, repo: string | null | undefined, path: string) => invoke<string>('git_worktree_open', { root, repo, path }),
  localRepoClose: (root: string, name: string) => invoke<void>('git_local_repo_close', { root, name }),

  // AI commit message
  stagedForAi: (root: string, repo: string | null | undefined) => invoke<StagedForAi>('git_staged_for_ai', { root, repo }),

  // pull / merge requests
  forgeInfo: (root: string, repo: string | null | undefined, remote?: string | null) => invoke<ForgeInfo>('git_forge_info', { root, repo, remote }),
  forgeAccounts: () => invoke<ForgeAccount[]>('git_forge_accounts'),
  /** Checks the token with the host, then keeps it in the OS keychain. */
  forgeConnect: (host: string, kind: ForgeKind, token: string, apiUrl?: string | null) => invoke<ForgeAccount>('git_forge_connect', { host, kind, token, apiUrl }),
  forgeDisconnect: (host: string) => invoke<void>('git_forge_disconnect', { host }),
  forgeCreate: (root: string, repo: string | null | undefined, remote: string, request: NewPullRequest, push: boolean, useCli: boolean) =>
    invoke<CreatedPullRequest>('git_forge_create', { root, repo, remote, request, push, useCli }),
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
