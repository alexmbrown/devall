import { create } from 'zustand';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import type { ForwardFile, ForwardStatus } from '@/lib/api/infra';
import { infraApi } from '@/lib/api/infra';
import { newRequestTemplate, useWorkspace } from '@/store/workspace';
import { useVarsVersion } from '@/store/variables';

export type InfraTab = 'docker' | 'kube' | 'forwards' | 'gcp';

const KEY = 'devall.infra.ui';

interface Persisted {
  tab: InfraTab;
  kubeContext?: string;
  kubeNamespace?: string;
  dockerHost?: string;
}

function load(): Persisted {
  try {
    return { tab: 'docker', ...(JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Persisted>) };
  } catch {
    return { tab: 'docker' };
  }
}

function save(p: Persisted) {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    /* storage unavailable */
  }
}

export interface ForwardDraft {
  spec: ForwardFile;
  /** Saved forward being edited. */
  id?: string;
}

interface InfraUi extends Persisted {
  set(patch: Partial<Persisted>): void;
  /** Open the forward dialog (from Kubernetes, Docker or the Forwards tab). */
  draft: ForwardDraft | null;
  openForward(draft: ForwardDraft): void;
  closeForward(): void;
  /** Bumped when forwards change, so lists reload. */
  forwardsVersion: number;
  bumpForwards(): void;
}

export const useInfraUi = create<InfraUi>((setState, get) => ({
  ...load(),
  set(patch) {
    setState(patch);
    const { tab, kubeContext, kubeNamespace, dockerHost } = { ...get(), ...patch };
    save({ tab, kubeContext, kubeNamespace, dockerHost });
  },
  draft: null,
  openForward: (draft) => setState({ draft }),
  closeForward: () => setState({ draft: null }),
  forwardsVersion: 0,
  bumpForwards: () => setState((s) => ({ forwardsVersion: s.forwardsVersion + 1 })),
}));

/** Creates a new HTTP request for `url` in the open project and switches to it. */
export async function openHttpRequest(url: string, name?: string) {
  const ws = useWorkspace.getState();
  if (!ws.project) {
    toast.error('Open a project first', 'Requests are saved in a project.');
    return;
  }
  await ws.newRequest('', { ...newRequestTemplate(name ?? `GET ${url.replace(/^https?:\/\//, '')}`), url });
}

/** Starts a forward and refreshes everything that shows variables. */
export async function startForward(root: string, id: string | undefined, spec?: ForwardFile): Promise<ForwardStatus | undefined> {
  try {
    const st = await infraApi.forward.start(root, id, spec);
    useVarsVersion.getState().bump();
    useInfraUi.getState().bumpForwards();
    const bound = st.binding ? ` · {{${st.binding.variable}}} = ${st.binding.value}` : '';
    toast.success(`Forwarding ${st.name}`, `${st.url ?? ''}${bound}`);
    return st;
  } catch (e) {
    toast.error(`Could not start ${spec?.name ?? id ?? 'forward'}`, errorMessage(e));
    return undefined;
  }
}

export async function stopForward(root: string, id: string) {
  try {
    await infraApi.forward.stop(root, id);
  } catch (e) {
    toast.error('Could not stop the forward', errorMessage(e));
  }
  useVarsVersion.getState().bump();
  useInfraUi.getState().bumpForwards();
}

/** A forward definition with sensible defaults. */
export function forwardTemplate(partial: Partial<ForwardFile> & Pick<ForwardFile, 'target' | 'remotePort'>): ForwardFile {
  const activeEnv = useWorkspace.getState().activeEnv();
  return {
    version: 1,
    name: partial.name ?? partial.target.name,
    ...partial,
    bindEnvironment: partial.bindEnvironment ?? { environment: activeEnv?.id, variable: 'baseUrl', template: 'http://127.0.0.1:{port}' },
  };
}

export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${u[i]}`;
}

/** `3m`, `2h`, `5d` since an RFC 3339 / unix-seconds time. */
export function age(created?: string | number): string {
  if (created === undefined || created === null || created === '') return '';
  const t = typeof created === 'number' ? created * 1000 : Date.parse(created);
  if (!Number.isFinite(t)) return '';
  const s = Math.max(0, Math.floor((Date.now() - t) / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86400)}d`;
}
