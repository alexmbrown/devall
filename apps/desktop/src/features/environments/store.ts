import { create } from 'zustand';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import type { EnvironmentEntry, EnvironmentFile, SecretStatus, VarUsage } from '@/lib/types';
import { useVarsVersion } from '@/store/variables';
import { useWorkspace } from '@/store/workspace';
import { referenceNames } from './matrix';

/**
 * Data shared by the environment editor, its inspector and the compare view: secret status of
 * every environment (names and where values come from — never values) and where each variable is
 * referenced. Keyed by project root; reloaded when environments change.
 */
interface EnvData {
  root: string | null;
  status: Record<string, SecretStatus[]>;
  usages: VarUsage[] | null;
  scanning: boolean;
  loadStatus(root: string, envIds: string[]): Promise<void>;
  loadUsages(root: string, envs: EnvironmentEntry[], projectVars?: Record<string, string>): Promise<void>;
}

let usageSeq = 0;

export const useEnvData = create<EnvData>((set, get) => ({
  root: null,
  status: {},
  usages: null,
  scanning: false,
  async loadStatus(root, envIds) {
    const pairs = await Promise.all(envIds.map(async (id) => [id, await api.secretStatus(root, id).catch(() => [] as SecretStatus[])] as const));
    const fresh = get().root !== root;
    set((s) => ({ root, status: { ...(fresh ? {} : s.status), ...Object.fromEntries(pairs) }, ...(fresh ? { usages: null } : {}) }));
  },
  async loadUsages(root, envs, projectVars) {
    const n = ++usageSeq;
    set({ scanning: true });
    try {
      const usages = await api.envUsages(root, referenceNames(envs, projectVars));
      if (n === usageSeq) set({ usages, root });
    } catch {
      if (n === usageSeq) set({ usages: [] });
    } finally {
      if (n === usageSeq) set({ scanning: false });
    }
  },
}));

/** Reloads everything for the open project (after a save, on mount). */
export function reloadEnvData() {
  const p = useWorkspace.getState().project;
  if (!p) return;
  const d = useEnvData.getState();
  void d.loadStatus(p.root, p.environments.map((e) => e.id));
  void d.loadUsages(p.root, p.environments, p.manifest.variables);
}

/** Saves one environment file (not the one being edited with unsaved changes) and refreshes. */
export async function saveEnvFile(id: string, file: EnvironmentFile, done?: string) {
  const p = useWorkspace.getState().project;
  if (!p) return false;
  try {
    await api.saveEnvironment(p.root, id, file);
    await useWorkspace.getState().refresh();
    useVarsVersion.getState().bump();
    if (done) toast.success(done);
    return true;
  } catch (e) {
    toast.error('Save failed', errorMessage(e));
    return false;
  }
}

/** Sets (or removes, with `value === undefined`) a variable in environment `id` on disk. */
export function setEnvVariable(entry: EnvironmentEntry, name: string, value: string | undefined) {
  const variables = { ...(entry.file.variables ?? {}) };
  if (value === undefined) delete variables[name];
  else variables[name] = value;
  return saveEnvFile(entry.id, { ...entry.file, variables }, value === undefined ? `${name} removed from ${entry.file.name}` : `${name} saved in ${entry.file.name}`);
}

/** Declares secret `name` in an environment that doesn't list it yet. */
export async function declareSecret(entry: EnvironmentEntry, name: string) {
  if ((entry.file.secrets ?? []).includes(name)) return true;
  return saveEnvFile(entry.id, { ...entry.file, secrets: [...(entry.file.secrets ?? []), name] });
}

/** Copies a keychain value between environments without it ever reaching the UI. */
export async function copySecretValue(fromEnv: string, to: EnvironmentEntry[], name: string, declare: (e: EnvironmentEntry, name: string) => Promise<boolean> = declareSecret) {
  const p = useWorkspace.getState().project;
  if (!p) return;
  try {
    for (const e of to) if (!(await declare(e, name))) return;
    const n = await api.transferSecrets(
      p.root,
      p.root,
      to.map((e) => ({ fromEnv, toEnv: e.id, name, exists: false })),
    );
    if (n) toast.success(`${name} copied to ${to.length === 1 ? to[0].file.name : `${n} environments`}`, 'Stored in your OS keychain.');
    else toast.info(`Nothing to copy for ${name}`, 'Its value here comes from a .env file, a secret manager or the shared all-environments value — not this environment’s keychain entry.');
    reloadEnvData();
  } catch (e) {
    toast.error(`Could not copy ${name}`, errorMessage(e));
  }
}
