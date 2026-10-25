import { useEffect } from 'react';
import { create } from 'zustand';
import { listen } from '@tauri-apps/api/event';
import { gitApi, type Repo, type RepoSummary } from '@/lib/api/git';
import { useFsVersion, useWorkspace } from './workspace';

/**
 * The open project's repositories (devall_core::repos, resolved on this machine) and, once source
 * control asked for them, their git summaries. Pickers (OpenAPI specs, notebook working folder,
 * log files…) only need `repos` (no git process); the Source control page refreshes `summaries`.
 */
interface ReposState {
  root: string | null;
  repos: Repo[];
  /** By repo name; `null` = not on this machine. Missing until an overview ran. */
  summaries: Record<string, RepoSummary | null>;
  loaded: boolean;
  /** Bumped by `git-changed` events (a watched repo's HEAD/index changed): names in `changed`. */
  changed: { v: number; repos: string[] };
  /** Re-reads the list (cheap: no git). */
  load(): Promise<Repo[]>;
  /** List + summaries (all, or only `names`, merged into the rest). */
  refreshOverview(names?: string[]): Promise<void>;
}

export const useRepos = create<ReposState>((set) => ({
  root: null,
  repos: [],
  summaries: {},
  loaded: false,
  changed: { v: 0, repos: [] },

  async load() {
    const root = useWorkspace.getState().project?.root;
    if (!root) return [];
    const repos = await gitApi.repos(root);
    if (useWorkspace.getState().project?.root === root) set({ root, repos, loaded: true });
    return repos;
  },

  async refreshOverview(names) {
    const root = useWorkspace.getState().project?.root;
    if (!root) return;
    const ov = await gitApi.overview(root, names);
    if (useWorkspace.getState().project?.root !== root) return;
    const fresh = Object.fromEntries(ov.map((o) => [o.repo.name, o.summary]));
    if (names) {
      const byName = new Map(ov.map((o) => [o.repo.name, o.repo]));
      set((s) => ({ repos: s.repos.map((r) => byName.get(r.name) ?? r), summaries: { ...s.summaries, ...fresh } }));
    } else {
      set({ root, repos: ov.map((o) => o.repo), summaries: fresh, loaded: true });
    }
  },
}));

/** The open project's repositories, loading the list on first use (no git process). */
export function useProjectRepos(): Repo[] {
  const root = useWorkspace((s) => s.project?.root ?? null);
  const repos = useRepos((s) => s.repos);
  const loadedFor = useRepos((s) => (s.loaded ? s.root : null));
  useEffect(() => {
    if (root && loadedFor !== root) void useRepos.getState().load().catch(() => {});
  }, [root, loadedFor]);
  return loadedFor === root ? repos : [];
}

/** The repo named `name` of the open project, if known. */
export function repoByName(name: string): Repo | undefined {
  return useRepos.getState().repos.find((r) => r.name === name) ?? useRepos.getState().repos.find((r) => r.name.toLowerCase() === name.toLowerCase());
}

// A different project: forget the previous one's repos and read the new list.
useWorkspace.subscribe((s, prev) => {
  const root = s.project?.root ?? null;
  if (root === (prev.project?.root ?? null)) return;
  useRepos.setState({ root, repos: [], summaries: {}, loaded: false });
  if (root) void useRepos.getState().load().catch(() => {});
});

// devall.yaml changed on disk (a teammate's repos arrived with a pull, the CLI edited them): re-read the list.
useFsVersion.subscribe((s, prev) => {
  if (s.v !== prev.v && s.paths.includes('devall.yaml') && useRepos.getState().loaded) void useRepos.getState().load().catch(() => {});
});

let listening = false;
/** Subscribes once to `git-changed` (emitted while source control watches the repos). */
export function listenGitChanges() {
  if (listening) return;
  listening = true;
  void listen<{ root: string; repos: string[] }>('git-changed', (e) => {
    if (e.payload.root !== useWorkspace.getState().project?.root) return;
    useRepos.setState((s) => ({ changed: { v: s.changed.v + 1, repos: e.payload.repos } }));
  }).catch(() => {
    listening = false;
  });
}
