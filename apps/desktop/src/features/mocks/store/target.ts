/**
 * Which WireMock the mock UI talks to: the project's built-in engine (`devall-mock`, started with
 * `mock_start`) or one of the remote servers saved in `mocks/servers.yaml`.
 */
import { useMemo } from 'react';
import { create } from 'zustand';
import { api, errorMessage } from '@/lib/ipc';
import { mockApi, type MockStatus } from '@/lib/api/mock';
import { toast } from '@/components/ui/toast';
import type { ServerAuth } from '@/features/mocks/lib/api';
import { keychainHas } from '@/features/mocks/lib/transport';
import { loadServers, saveServers as writeServers, secretNameFor, secretProblems, type EnvKind, type RemoteServer } from '@/features/mocks/lib/servers';

export { ENV_LABEL, type EnvKind, type RemoteServer } from '@/features/mocks/lib/servers';

export const BUILTIN_ID = 'builtin';

export interface Target {
  kind: 'builtin' | 'remote';
  /** `builtin` or the remote server id. */
  id: string;
  name: string;
  /** Base URL; empty while the built-in engine is stopped. */
  url: string;
  contextPath?: string;
  env?: EnvKind;
  readOnly?: boolean;
  protected?: boolean;
  auth?: ServerAuth;
  remote?: RemoteServer;
  /** Project root (built-in engine only). */
  root?: string;
}

export interface BuiltinPrefs {
  port?: number;
  bindAll?: boolean;
  /** Start the engine when the mock page opens. */
  autoStart?: boolean;
}

interface TargetState {
  root: string | null;
  status: MockStatus | null;
  statusLoaded: boolean;
  starting: boolean;
  servers: RemoteServer[];
  serversLoaded: boolean;
  serversExists: boolean;
  serversWarnings: string[];
  activeId: string;
  /** Session-only secrets by server id (memory only, never persisted). */
  sessionSecrets: Record<string, string>;
  /** Whether the keychain holds the server's secret (undefined = unknown). */
  secretPresent: Record<string, boolean | undefined>;
  /** Time of the last write this UI made to the built-in engine (to ignore our own file events). */
  lastLocalWrite: number;
  /** `mocks/mappings` changed on disk outside this UI. */
  diskChanged: boolean;

  /** Loads the project's mock target; starts the built-in server when it's set to auto-start (unless `autoStart: false`). */
  init(root: string | null, opts?: { autoStart?: boolean }): Promise<void>;
  refreshStatus(): Promise<MockStatus | null>;
  start(port?: number, bindAll?: boolean): Promise<boolean>;
  stop(): Promise<void>;
  setActive(id: string): void;
  reloadServers(): Promise<void>;
  saveServers(servers: RemoteServer[]): Promise<boolean>;
  setSessionSecret(id: string, value?: string): void;
  refreshSecret(id: string): Promise<void>;
  storeSecret(id: string, value: string): Promise<boolean>;
  deleteSecret(id: string): Promise<void>;
  markLocalWrite(): void;
  setDiskChanged(v: boolean): void;
}

const ACTIVE_KEY = (root: string) => `devall.mocks.active:${root}`;
const BUILTIN_KEY = (root: string) => `devall.mocks.builtin:${root}`;

function readLocal<T>(key: string, fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    return v ? (JSON.parse(v) as T) : fallback;
  } catch {
    return fallback;
  }
}

function writeLocal(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v));
  } catch {
    /* storage unavailable */
  }
}

export function builtinPrefs(root: string | null): BuiltinPrefs {
  return root ? readLocal<BuiltinPrefs>(BUILTIN_KEY(root), {}) : {};
}

export function setBuiltinPrefs(root: string, patch: Partial<BuiltinPrefs>) {
  writeLocal(BUILTIN_KEY(root), { ...builtinPrefs(root), ...patch });
}

export const useMockTarget = create<TargetState>((set, get) => ({
  root: null,
  status: null,
  statusLoaded: false,
  starting: false,
  servers: [],
  serversLoaded: false,
  serversExists: false,
  serversWarnings: [],
  activeId: BUILTIN_ID,
  sessionSecrets: {},
  secretPresent: {},
  lastLocalWrite: 0,
  diskChanged: false,

  async init(root, opts) {
    if (root === get().root) return;
    set({
      root,
      status: null,
      statusLoaded: false,
      servers: [],
      serversLoaded: false,
      serversExists: false,
      serversWarnings: [],
      activeId: root ? readLocal<string>(ACTIVE_KEY(root), BUILTIN_ID) : BUILTIN_ID,
      sessionSecrets: {},
      secretPresent: {},
      diskChanged: false,
    });
    if (!root) return;
    await Promise.all([get().refreshStatus(), get().reloadServers()]);
    if (get().root !== root) return;
    const prefs = builtinPrefs(root);
    if (prefs.autoStart && opts?.autoStart !== false && !get().status?.running) await get().start(prefs.port, prefs.bindAll);
  },

  async refreshStatus() {
    const root = get().root;
    if (!root) return null;
    try {
      const status = await mockApi.status(root);
      if (get().root !== root) return status;
      // Keep the object identity when nothing changed (polling must not re-render everything).
      if (JSON.stringify(status) === JSON.stringify(get().status)) set({ statusLoaded: true });
      else set({ status, statusLoaded: true });
      return status;
    } catch (e) {
      if (get().root === root) set({ statusLoaded: true });
      toast.error('Mock server status unavailable', errorMessage(e));
      return null;
    }
  },

  async start(port, bindAll) {
    const root = get().root;
    if (!root) return false;
    set({ starting: true });
    try {
      const status = await mockApi.start(root, port || undefined, !!bindAll);
      setBuiltinPrefs(root, { port: port || undefined, bindAll: !!bindAll });
      if (get().root === root) set({ status, diskChanged: false });
      return true;
    } catch (e) {
      toast.error('Could not start the mock server', errorMessage(e));
      return false;
    } finally {
      set({ starting: false });
    }
  },

  async stop() {
    const root = get().root;
    if (!root) return;
    try {
      await mockApi.stop(root);
    } catch (e) {
      toast.error('Could not stop the mock server', errorMessage(e));
    }
    await get().refreshStatus();
  },

  setActive(id) {
    const root = get().root;
    set({ activeId: id });
    if (root) writeLocal(ACTIVE_KEY(root), id);
  },

  async reloadServers() {
    const root = get().root;
    if (!root) return;
    try {
      const r = await loadServers(root);
      if (get().root !== root) return;
      set({ servers: r.servers, serversExists: r.exists, serversWarnings: r.warnings, serversLoaded: true });
      for (const s of r.servers) if (s.auth && s.auth.type !== 'none') void get().refreshSecret(s.id);
    } catch (e) {
      if (get().root === root) set({ serversLoaded: true, serversWarnings: [`mocks/servers.yaml: ${errorMessage(e)}`] });
    }
  },

  async saveServers(servers) {
    const root = get().root;
    if (!root) return false;
    const problems = secretProblems(servers);
    if (problems.length) {
      toast.error('Not saved: secrets do not belong in servers.yaml', problems.join('\n'));
      return false;
    }
    try {
      await writeServers(root, servers);
      set({ servers, serversExists: true, serversWarnings: [] });
      if (!servers.some((s) => s.id === get().activeId) && get().activeId !== BUILTIN_ID) get().setActive(BUILTIN_ID);
      return true;
    } catch (e) {
      toast.error('Could not save mocks/servers.yaml', errorMessage(e));
      return false;
    }
  },

  setSessionSecret(id, value) {
    const next = { ...get().sessionSecrets };
    if (value === undefined) delete next[id];
    else next[id] = value;
    set({ sessionSecrets: next });
  },

  async refreshSecret(id) {
    const root = get().root;
    if (!root) return;
    const has = await keychainHas(root, secretNameFor(id));
    set({ secretPresent: { ...get().secretPresent, [id]: has } });
  },

  async storeSecret(id, value) {
    const root = get().root;
    if (!root) return false;
    try {
      await api.setSecret(root, '*', secretNameFor(id), value);
      set({ secretPresent: { ...get().secretPresent, [id]: true } });
      return true;
    } catch (e) {
      toast.error('Could not save to the keychain', errorMessage(e));
      return false;
    }
  },

  async deleteSecret(id) {
    const root = get().root;
    if (!root) return;
    try {
      await api.deleteSecret(root, '*', secretNameFor(id));
      set({ secretPresent: { ...get().secretPresent, [id]: false } });
    } catch (e) {
      toast.error('Could not remove the keychain entry', errorMessage(e));
    }
  },

  markLocalWrite() {
    set({ lastLocalWrite: Date.now() });
  },

  setDiskChanged(v) {
    set({ diskChanged: v });
  },
}));

export function builtinTarget(status: MockStatus | null, root?: string | null): Target {
  return { kind: 'builtin', id: BUILTIN_ID, name: 'This project', url: status?.running && status.url ? status.url : '', env: 'local', root: root ?? status?.root };
}

export function remoteTarget(s: RemoteServer, sessionSecret?: string): Target {
  return {
    kind: 'remote',
    id: s.id,
    name: s.name,
    url: s.url,
    contextPath: s.contextPath,
    env: s.env,
    readOnly: s.readOnly,
    protected: s.protected,
    auth: s.auth && s.auth.type !== 'none' ? { ...s.auth, secret: secretNameFor(s.id), value: sessionSecret } : undefined,
    remote: s,
  };
}

/** Every target: the built-in engine first, then the saved remote servers. */
export function useTargets(): Target[] {
  const status = useMockTarget((s) => s.status);
  const root = useMockTarget((s) => s.root);
  const servers = useMockTarget((s) => s.servers);
  const secrets = useMockTarget((s) => s.sessionSecrets);
  return useMemo(() => [builtinTarget(status, root), ...servers.map((s) => remoteTarget(s, secrets[s.id]))], [status, root, servers, secrets]);
}

export function useActiveTarget(): Target {
  const targets = useTargets();
  const activeId = useMockTarget((s) => s.activeId);
  return targets.find((t) => t.id === activeId) ?? targets[0];
}

export function targetById(id: string): Target | undefined {
  const st = useMockTarget.getState();
  if (id === BUILTIN_ID) return builtinTarget(st.status, st.root);
  const s = st.servers.find((x) => x.id === id);
  return s ? remoteTarget(s, st.sessionSecrets[s.id]) : undefined;
}

export function activeTargetOf(): Target {
  const st = useMockTarget.getState();
  return targetById(st.activeId) ?? builtinTarget(st.status, st.root);
}

/**
 * Cache / tab key for a target. Remote keys change with the URL so stale data is never shown; the
 * built-in engine keeps one key per project (its port changes between runs; queries are
 * invalidated when it restarts).
 */
export function serverKey(t?: Target): string {
  if (!t) return 'none';
  if (t.kind === 'builtin') return `builtin@${t.root ?? ''}`;
  return `${t.id}@${t.url.trim().replace(/\/+$/, '').toLowerCase()}${t.contextPath ?? ''}`;
}
