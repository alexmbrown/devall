import { create } from 'zustand';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type ChangedFile, type CompareMode, type Comparison, type ConflictPrediction, type DiffSide, type RangeFiles, type Reviewed } from '@/lib/api/git';
import { applyMarks, displayOrder, markChanges, nextUnreviewed, reviewState, visibleFiles } from './review';

/**
 * The comparison shown by Source control's Compare tab (a singleton section, so one global store):
 * what is compared, the loaded files/commits, conflict prediction, reviewed marks, the selected
 * file or commit range and the view options. The side pane (files/commits) and the main pane
 * (header + diff) both read it.
 */
export interface CompareSpec {
  base: string;
  head: string;
  mode: CompareMode;
  uncommitted: boolean;
}

export interface CompareOpts {
  layout: 'split' | 'unified';
  ignoreWs: boolean;
  wrap: boolean;
  /** Whole files instead of changed regions only. */
  full: boolean;
  tree: boolean;
  hideReviewed: boolean;
  onlyChanged: boolean;
  showGenerated: boolean;
}

const OPTS_KEY = 'devall.git.compare.opts';
const DEFAULT_OPTS: CompareOpts = { layout: 'split', ignoreWs: false, wrap: false, full: false, tree: true, hideReviewed: false, onlyChanged: false, showGenerated: false };

function readOpts(): CompareOpts {
  try {
    return { ...DEFAULT_OPTS, ...(JSON.parse(localStorage.getItem(OPTS_KEY) ?? '{}') as Partial<CompareOpts>), onlyChanged: false };
  } catch {
    return DEFAULT_OPTS;
  }
}

const baseKey = (root: string, repo: string | null) => `devall.git.compareBase:${root}|${repo ?? ''}`;

/** The base last compared against in this repo (per device). */
export function rememberedBase(root: string, repo: string | null): string | null {
  try {
    return localStorage.getItem(baseKey(root, repo));
  } catch {
    return null;
  }
}

export function rememberBase(root: string, repo: string | null, base: string) {
  try {
    localStorage.setItem(baseKey(root, repo), base);
  } catch {
    /* storage unavailable */
  }
}

interface CompareState {
  root: string | null;
  repo: string | null;
  spec: CompareSpec | null;
  data: Comparison | null;
  loading: boolean;
  error: string | null;
  conflicts: ConflictPrediction | null;
  conflictsLoading: boolean;
  marks: Record<string, Reviewed>;
  /** Selected file path. */
  selected: string | null;
  /** Diff the selected file against the version that was reviewed. */
  sinceReview: boolean;
  /** Selected commits (a run in list order); empty = the whole comparison. */
  commitSel: string[];
  range: RangeFiles | null;
  rangeLoading: boolean;
  tab: 'files' | 'commits';
  filter: string;
  opts: CompareOpts;
  /** Bumped by `load` so late answers are dropped. */
  seq: number;

  load(root: string, repo: string | null, spec: CompareSpec, force?: boolean): Promise<void>;
  refresh(): Promise<void>;
  select(path: string | null): void;
  setTab(tab: 'files' | 'commits'): void;
  setFilter(filter: string): void;
  setOpts(patch: Partial<CompareOpts>): void;
  setSinceReview(v: boolean): void;
  selectCommits(hashes: string[]): Promise<void>;
  setReviewed(files: ChangedFile[], mark: boolean): Promise<void>;
  clear(): void;
}

const sameSpec = (a: CompareSpec | null, b: CompareSpec) => !!a && a.base === b.base && a.head === b.head && a.mode === b.mode && a.uncommitted === b.uncommitted;

export const useCompare = create<CompareState>((set, get) => ({
  root: null,
  repo: null,
  spec: null,
  data: null,
  loading: false,
  error: null,
  conflicts: null,
  conflictsLoading: false,
  marks: {},
  selected: null,
  sinceReview: false,
  commitSel: [],
  range: null,
  rangeLoading: false,
  tab: 'files',
  filter: '',
  opts: readOpts(),
  seq: 0,

  async load(root, repo, spec, force) {
    const s = get();
    // Same refs: keep the selection and marks (a mode or uncommitted switch just re-reads).
    const sameTarget = s.root === root && s.repo === repo && !!s.spec && s.spec.base === spec.base && s.spec.head === spec.head;
    if (sameTarget && sameSpec(s.spec, spec) && !force && (s.data || s.loading)) return;
    const seq = s.seq + 1;
    set({
      root,
      repo,
      spec,
      seq,
      loading: true,
      error: null,
      ...(sameTarget ? {} : { data: null, conflicts: null, marks: {}, selected: null, commitSel: [], range: null, sinceReview: false }),
    });
    try {
      const [data, marks] = await Promise.all([gitApi.compare(root, repo, spec.base, spec.head, spec.mode, spec.uncommitted), gitApi.reviewGet(root, repo, spec.base, spec.head).catch(() => ({}))]);
      if (get().seq !== seq) return;
      // Remember branch bases (not commits picked in the graph) as this repo's default.
      if (data.base.kind === 'branch' || data.base.kind === 'remote') rememberBase(root, repo, spec.base);
      const cur = get();
      const keep = cur.selected && data.files.some((f) => f.path === cur.selected) ? cur.selected : null;
      const order = displayOrder(data.files, cur.opts.tree);
      const first = keep ?? nextUnreviewed([...order.main, ...order.generated], marks, null) ?? order.main[0]?.path ?? null;
      set({ data, marks, loading: false, selected: first });
      // Commit selection survives a refresh only while those commits are still listed.
      if (cur.commitSel.length && !cur.commitSel.every((h) => data.commits.some((c) => c.hash === h))) set({ commitSel: [], range: null });
    } catch (e) {
      if (get().seq === seq) set({ loading: false, error: errorMessage(e), data: null });
      return;
    }
    set({ conflictsLoading: true });
    try {
      const conflicts = await gitApi.compareConflicts(root, repo, spec.base, spec.head);
      if (get().seq === seq) set({ conflicts, conflictsLoading: false });
    } catch (e) {
      if (get().seq === seq) set({ conflicts: { supported: true, clean: false, files: [], message: errorMessage(e), gitVersion: '' }, conflictsLoading: false });
    }
  },

  async refresh() {
    const { root, repo, spec } = get();
    if (root && spec) await get().load(root, repo, spec, true);
  },

  select: (path) => set({ selected: path, sinceReview: false }),
  setTab: (tab) => set({ tab }),
  setFilter: (filter) => set({ filter }),
  setOpts(patch) {
    const opts = { ...get().opts, ...patch };
    set({ opts });
    try {
      localStorage.setItem(OPTS_KEY, JSON.stringify(opts));
    } catch {
      /* storage unavailable */
    }
  },
  setSinceReview: (sinceReview) => set({ sinceReview }),

  async selectCommits(hashes) {
    const { root, repo, data } = get();
    if (!root || !data) return;
    if (!hashes.length) return set({ commitSel: [], range: null, rangeLoading: false });
    // Keep list order (newest first): the range runs from the oldest's parent to the newest.
    const ordered = data.commits.filter((c) => hashes.includes(c.hash)).map((c) => c.hash);
    set({ commitSel: ordered, rangeLoading: true });
    const seq = get().seq;
    try {
      const range = await gitApi.commitRange(root, repo, ordered[ordered.length - 1], ordered[0]);
      if (get().seq !== seq || get().commitSel.join() !== ordered.join()) return;
      const keep = get().selected && range.files.some((f) => f.path === get().selected) ? get().selected : (range.files[0]?.path ?? null);
      set({ range, rangeLoading: false, selected: keep });
    } catch (e) {
      if (get().seq === seq) set({ rangeLoading: false, error: errorMessage(e) });
    }
  },

  async setReviewed(files, mark) {
    const { root, repo, spec, marks } = get();
    if (!root || !spec || !files.length) return;
    const changes = markChanges(files, mark);
    set({ marks: applyMarks(marks, changes, Math.floor(Date.now() / 1000)) });
    try {
      const fresh = await gitApi.reviewSet(root, repo, spec.base, spec.head, changes);
      if (get().spec === spec) set({ marks: fresh });
    } catch (e) {
      set({ marks });
      throw e;
    }
  },

  clear: () => set({ root: null, repo: null, spec: null, data: null, error: null, conflicts: null, marks: {}, selected: null, commitSel: [], range: null }),
}));

/** The files listed right now: a commit range's, or the comparison's. */
export function listedFiles(s: Pick<CompareState, 'data' | 'range' | 'commitSel'>): ChangedFile[] {
  if (s.commitSel.length) return s.range?.files ?? [];
  return s.data?.files ?? [];
}

/** Files in the order the list shows them (filtered), regular ones then generated. */
export function navOrder(s: Pick<CompareState, 'data' | 'range' | 'commitSel' | 'marks' | 'filter' | 'opts'>): { main: ChangedFile[]; generated: ChangedFile[] } {
  const reviewing = !s.commitSel.length;
  const files = visibleFiles(listedFiles(s), reviewing ? s.marks : {}, { text: s.filter, hideReviewed: reviewing && s.opts.hideReviewed, onlyChanged: reviewing && s.opts.onlyChanged });
  return displayOrder(files, s.opts.tree);
}

/** The two sides of the selected file's diff. */
export function diffSides(s: Pick<CompareState, 'data' | 'range' | 'commitSel' | 'marks' | 'sinceReview'>, f: ChangedFile): { old: DiffSide; new: DiffSide; label: string } | null {
  const data = s.data;
  if (!data) return null;
  const ranged = s.commitSel.length > 0 && s.range;
  const from = ranged ? s.range!.from : data.from;
  const to = ranged ? s.range!.to : data.to;
  const newSide: DiffSide = f.status === 'D' ? { type: 'empty' } : to ? { type: 'rev', rev: to, path: f.path } : { type: 'worktree', path: f.path };
  const mark = s.marks[f.path];
  if (!ranged && s.sinceReview && mark && reviewState(f, s.marks) === 'changed' && !mark.blob.startsWith('-')) return { old: { type: 'blob', id: mark.blob }, new: newSide, label: 'since your review' };
  const oldSide: DiffSide = f.status === 'A' || f.untracked ? { type: 'empty' } : { type: 'rev', rev: from, path: f.oldPath ?? f.path };
  return { old: oldSide, new: newSide, label: ranged ? `${s.commitSel.length} commit${s.commitSel.length === 1 ? '' : 's'}` : data.mode === 'mergeBase' ? 'since branched' : 'tip to tip' };
}
