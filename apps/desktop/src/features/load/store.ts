import { create } from 'zustand';
import { loadApi, type CloudRun, type DistributedRun, type LoadSnapshot, type LoadSummary, type TimelinePoint } from '@/lib/api/load';
import { errorMessage } from '@/lib/ipc';
import { uid } from '@/lib/id';
import { toast } from '@/components/ui/toast';
import { confirmDialog } from '@/components/ui/dialogs';

/** A run in progress. Lives here (not in a component) so it survives navigating away. */
export interface LiveRun {
  runId: string;
  snapshot?: LoadSnapshot;
  points: TimelinePoint[];
  stopping: boolean;
}

export interface FinishedRun {
  summary: LoadSummary;
  /** Summary file relative to the project root, when it was written. */
  file: string | null;
}

interface LoadUiState {
  /** Keyed by `${root}|${path}`. */
  live: Record<string, LiveRun>;
  /** The run currently shown in Results, per test. */
  shown: Record<string, FinishedRun>;
  /** Bumped whenever runs are written/deleted so lists reload. */
  runsVersion: number;
  /**
   * `againstMock`: send the load to the project's mock server (saved as a mock baseline).
   * `distributed`: split it across load agents instead of running it here.
   * `cloud`: run it from the relay's cloud regions (verified targets only).
   */
  start(root: string, path: string, env: string | undefined, againstMock?: boolean, distributed?: DistributedRun, cloud?: CloudRun): Promise<void>;
  stop(root: string, path: string): Promise<void>;
  show(root: string, path: string, run: FinishedRun | undefined): void;
  bumpRuns(): void;
}

export const runKey = (root: string, path: string) => `${root}|${path}`;

export const useLoadRuns = create<LoadUiState>((set, get) => ({
  live: {},
  shown: {},
  runsVersion: 0,

  async start(root, path, env, againstMock, distributed, cloud) {
    const key = runKey(root, path);
    if (get().live[key]) return;
    const runId = uid();
    const onProgress = (s: LoadSnapshot) =>
      set((st) => {
        const cur = st.live[key];
        if (!cur || cur.runId !== s.runId) return st;
        return { live: { ...st.live, [key]: { ...cur, snapshot: s, points: s.points.length ? [...cur.points, ...s.points] : cur.points } } };
      });
    const begin = () => set((st) => ({ live: { ...st.live, [key]: { runId, points: [], stopping: false } } }));
    const end = () =>
      set((st) => {
        const live = { ...st.live };
        delete live[key];
        return { live };
      });
    const send = (confirm: boolean) =>
      cloud
        ? loadApi.runCloud(root, path, runId, env, cloud, onProgress)
        : distributed
          ? loadApi.runDistributed(root, path, runId, env, confirm, distributed, onProgress)
          : loadApi.run(root, path, runId, env, confirm, onProgress, againstMock);
    begin();
    try {
      let result = await send(false);
      if (result.type === 'requiresConfirmation') {
        end();
        const c = result.confirmation;
        const ok = await confirmDialog({
          title: 'Run this load test?',
          message: c.reason,
          confirmText: 'Start load test',
          danger: true,
          typeToConfirm: c.hosts[0],
        });
        if (!ok) return;
        begin();
        result = await send(true);
      }
      end();
      if (result.type !== 'completed') return;
      const { summary, file, writeError } = result;
      set((st) => ({ shown: { ...st.shown, [key]: { summary, file } }, runsVersion: st.runsVersion + 1 }));
      if (writeError) toast.error('Could not save the run summary', writeError);
      const failedAgents = (summary.agents ?? []).filter((a) => !a.ok);
      const unit = cloud ? 'region' : 'agent';
      if (failedAgents.length) toast.error(`${failedAgents.length} ${unit}${failedAgents.length === 1 ? '' : 's'} failed`, failedAgents.map((a) => `${a.address}: ${a.error ?? 'no results'}`).join('\n'));
      if (summary.mockBaseline) toast.success(`${summary.test}: mock baseline saved`, `p95 ${summary.latency.p95.toFixed(1)} ms against ${summary.mockBaseline.url} — compare a real run with it in History`);
      else if (summary.aborted) toast.info('Load test stopped', summary.abortReason);
      else if (summary.thresholdsPassed) toast.success(`${summary.test} passed`, `${summary.thresholds.length} thresholds held`);
      else toast.error(`${summary.test} failed`, summary.thresholds.filter((t) => !t.ok).map((t) => t.message).join('\n') || undefined);
    } catch (e) {
      end();
      toast.error(cloud ? 'Cloud run failed' : distributed ? 'Distributed run failed' : 'Load test failed to start', errorMessage(e));
    }
  },

  async stop(root, path) {
    const key = runKey(root, path);
    const cur = get().live[key];
    if (!cur) return;
    set((st) => ({ live: { ...st.live, [key]: { ...cur, stopping: true } } }));
    await loadApi.stop(cur.runId).catch(() => false);
  },

  show(root, path, run) {
    const key = runKey(root, path);
    set((st) => {
      const shown = { ...st.shown };
      if (run) shown[key] = run;
      else delete shown[key];
      return { shown };
    });
  },

  bumpRuns() {
    set((st) => ({ runsVersion: st.runsVersion + 1 }));
  },
}));
