import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api, errorMessage, scanApi } from '@/lib/ipc';
import { uid } from '@/lib/id';
import { toast } from '@/components/ui/toast';
import { confirmDialog, saveChangesDialog } from '@/components/ui/dialogs';
import { usePrefs } from './prefs';
import { useNav, type GoOptions } from './nav';
import { registerCloseGuard, useWorkbench, type ViewSpec } from './workbench';
import type { AppInfo, EnvironmentEntry, FolderFile, HttpRequestFile, ProjectInfo, SendResult, SseEvent, StreamMsg, TreeNode } from '@/lib/types';
import { useVarsVersion } from './variables';
import { protoKindOf, type ProtoKind } from '@/lib/api/proto';

/** Bumped after every send so history lists refresh. */
export const useHistoryVersion = create<{ v: number; bump(): void }>((set) => ({ v: 0, bump: () => set((s) => ({ v: s.v + 1 })) }));

export interface RequestTab {
  id: string;
  kind: 'request';
  path: string;
  draft: HttpRequestFile;
  /** Serialized last-saved state, for dirty tracking. */
  saved: string;
  result?: SendResult;
  /** Set while a send is in flight. */
  sendId?: string;
  /** Server-Sent Events received during the latest send (live). */
  stream?: SseEvent[];
  /** Bytes received so far during the latest send. */
  progress?: number;
}

const MAX_STREAM_EVENTS = 5000;

export interface FolderTab {
  id: string;
  kind: 'folder';
  path: string;
  draft: FolderFile;
  saved: string;
}

/** WebSocket / gRPC / MQTT / Socket.IO file — its editor loads and autosaves the file itself. */
export interface ProtoTab {
  id: string;
  kind: 'proto';
  path: string;
  protocol: ProtoKind;
  name: string;
  draft: null;
  saved: string;
}

/**
 * An open document of the HTTP module. Documents are keyed by path and shown by workbench views of
 * section `http` whose `param` is {@link docParam} — several views may show different documents
 * (or the same one) side by side. A document is loaded when a view needs it and released when no
 * view shows it any more (see `useWorkbench.subscribe` below).
 */
export type Tab = RequestTab | FolderTab | ProtoTab;

const FOLDER_FILE = '_folder.yaml';

/** What an `http` view param points at: a request, a WebSocket/gRPC/MQTT/Socket.IO file, or folder settings. */
export function parseDocParam(param: string): { kind: Tab['kind']; path: string; protocol?: ProtoKind } {
  if (param === FOLDER_FILE || param.endsWith(`/${FOLDER_FILE}`)) return { kind: 'folder', path: param.slice(0, Math.max(0, param.length - FOLDER_FILE.length - 1)) };
  const protocol = protoKindOf(param);
  return protocol ? { kind: 'proto', path: param, protocol } : { kind: 'request', path: param };
}

/** The `http` view param of a document (folder settings use the folder's `_folder.yaml`). */
export function docParam(t: Pick<Tab, 'kind' | 'path'>): string {
  return t.kind === 'folder' ? (t.path ? `${t.path}/${FOLDER_FILE}` : FOLDER_FILE) : t.path;
}

/** The loaded document an `http` view param refers to. */
export function docForParam(tabs: Tab[], param: string | undefined): Tab | undefined {
  if (!param) return undefined;
  const { kind, path } = parseDocParam(param);
  return tabs.find((t) => t.kind === kind && t.path === path);
}

/** Ends a proto tab's live connection (lazy import keeps the store free of feature code). */
function closeLiveSession(root: string, path: string) {
  import('@/features/proto').then((m) => m.closeProtoSession(root, path)).catch(() => {});
}

const SAFE_METHODS = ['GET', 'HEAD', 'OPTIONS'];
const AUTOSAVE_MS = 600;
const autosaveTimers = new Map<string, ReturnType<typeof setTimeout>>();

export function newRequestTemplate(name = 'New request'): HttpRequestFile {
  return { id: '', name, method: 'GET', url: '{{baseUrl}}/' };
}

export const isDirty = (t: Tab) => JSON.stringify(t.draft) !== t.saved;

interface WorkspaceState {
  app?: AppInfo;
  project: ProjectInfo | null;
  tree: TreeNode | null;
  ready: boolean;
  /** Open documents (see {@link Tab}). */
  tabs: Tab[];

  init(): Promise<void>;
  openProject(path: string): Promise<boolean>;
  openScratch(): Promise<void>;
  createProject(path: string, name: string): Promise<boolean>;
  closeProject(): void;
  refresh(): Promise<void>;

  activeEnv(): EnvironmentEntry | undefined;
  setActiveEnv(id: string | null): void;

  /** Opens a request in the workbench (a preview tab unless `{ preview: false }`). */
  openRequest(path: string, opts?: GoOptions): Promise<void>;
  newRequest(folder: string, template?: HttpRequestFile): Promise<string | undefined>;
  /** Opens a folder's settings (`_folder.yaml`) in the workbench. */
  openFolder(path: string, opts?: GoOptions): Promise<void>;
  openProto(path: string, protocol: ProtoKind, name: string, opts?: GoOptions): void;
  /** Loads the document an `http` view param refers to (once; concurrent calls share the load). */
  ensureDoc(param: string): Promise<Tab | undefined>;
  /** Closes every view showing this document (asking to save when it has unsaved changes). */
  closeTab(id: string): Promise<void>;
  updateRequest(tabId: string, fn: (d: HttpRequestFile) => HttpRequestFile): void;
  updateFolder(tabId: string, fn: (d: FolderFile) => FolderFile): void;
  save(tabId: string): Promise<boolean>;
  rename(path: string, name: string): Promise<string | undefined>;
  remove(path: string): Promise<void>;
  move(path: string, target: string, index?: number): Promise<void>;
  send(tabId: string): Promise<void>;
  cancel(tabId: string): Promise<void>;
  /** Closes (without saving) the tabs of items that left the project, e.g. after “Move to project”. */
  dropTabs(paths: string[]): void;
}

/** Pre-workbench tab list (migrated into the workbench layout the first time a project opens). */
const PROJECT_TABS_KEY = (root: string) => `devall.tabs:${root}`;
const loading = new Map<string, Promise<Tab | undefined>>();

/** Re-points open tabs after a file/folder moved from `from` to `to`. */
function repath(tabs: Tab[], from: string, to: string): Tab[] {
  return tabs.map((t) => {
    if (t.path === from) return { ...t, path: to };
    if (t.path.startsWith(`${from}/`)) return { ...t, path: to + t.path.slice(from.length) };
    return t;
  });
}

export const useWorkspace = create<WorkspaceState>((set, get) => ({
  project: null,
  tree: null,
  ready: false,
  tabs: [],

  async init() {
    try {
      const app = await api.appInfo();
      set({ app });
      const last = usePrefs.getState().lastProject;
      if (last && (await get().openProject(last))) return;
      await get().openScratch();
    } catch (e) {
      toast.error('Could not start', errorMessage(e));
    } finally {
      set({ ready: true });
    }
  },

  async openProject(path) {
    try {
      const project = await api.openProject(path);
      await loadProject(project);
      return true;
    } catch (e) {
      toast.error('Could not open project', errorMessage(e));
      return false;
    }
  },

  async openScratch() {
    const project = await api.openScratch();
    await loadProject(project);
  },

  async createProject(path, name) {
    try {
      const project = await api.createProject(path, name);
      await loadProject(project);
      toast.success(`Created ${project.manifest.name}`, project.root);
      return true;
    } catch (e) {
      toast.error('Could not create project', errorMessage(e));
      return false;
    }
  },

  closeProject() {
    for (const t of get().tabs) releaseDoc(t, get().project?.root);
    set({ project: null, tree: null, tabs: [] });
    usePrefs.getState().set('lastProject', null);
    useWorkbench.getState().load(null);
    useNav.getState().go('projects');
  },

  async refresh() {
    const p = get().project;
    if (!p) return;
    try {
      const [tree, environments] = await Promise.all([api.tree(p.root), api.environments(p.root)]);
      set({ tree, project: { ...p, environments } });
      // Reload clean tabs whose file changed on disk (e.g. after `git pull`).
      for (const t of get().tabs) {
        if (t.kind !== 'request' || isDirty(t)) continue;
        api
          .readRequest(p.root, t.path)
          .then((fresh) => {
            const s = JSON.stringify(fresh);
            if (s !== t.saved) set((st) => ({ tabs: st.tabs.map((x) => (x.id === t.id && x.kind === 'request' ? { ...x, draft: fresh, saved: s } : x)) }));
          })
          .catch(() => {});
      }
    } catch (e) {
      toast.error('Refresh failed', errorMessage(e));
    }
  },

  activeEnv() {
    const p = get().project;
    if (!p) return undefined;
    const id = usePrefs.getState().activeEnv[p.root];
    return p.environments.find((e) => e.id === id);
  },

  setActiveEnv(id) {
    const p = get().project;
    if (!p) return;
    const map = { ...usePrefs.getState().activeEnv };
    if (id) map[p.root] = id;
    else delete map[p.root];
    usePrefs.getState().set('activeEnv', map);
  },

  async openRequest(path, opts) {
    if (await get().ensureDoc(path)) useWorkbench.getState().open('http', path, opts);
  },

  async newRequest(folder, template) {
    const p = get().project!;
    try {
      const path = await api.createRequest(p.root, folder, template ?? newRequestTemplate());
      await get().refresh();
      await get().openRequest(path, { preview: false });
      return path;
    } catch (e) {
      toast.error('Could not create request', errorMessage(e));
    }
  },

  async openFolder(path, opts) {
    const param = docParam({ kind: 'folder', path });
    if (await get().ensureDoc(param)) useWorkbench.getState().open('http', param, { preview: false, ...opts });
  },

  openProto(path, protocol, name, opts) {
    if (!get().tabs.some((t) => t.kind === 'proto' && t.path === path)) set((s) => ({ tabs: [...s.tabs, { id: uid(), kind: 'proto', path, protocol, name, draft: null, saved: 'null' }] }));
    useWorkbench.getState().open('http', path, opts);
  },

  ensureDoc(param) {
    const existing = docForParam(get().tabs, param);
    if (existing) return Promise.resolve(existing);
    const p = get().project;
    if (!p) return Promise.resolve(undefined);
    const key = `${p.root}|${param}`;
    const inflight = loading.get(key);
    if (inflight) return inflight;
    const { kind, path, protocol } = parseDocParam(param);
    const job = (async (): Promise<Tab | undefined> => {
      try {
        let tab: Tab;
        if (kind === 'folder') {
          const draft = await api.readFolder(p.root, path);
          tab = { id: uid(), kind: 'folder', path, draft, saved: JSON.stringify(draft) };
        } else if (kind === 'proto') {
          const node = findTreeNode(get().tree, path);
          tab = { id: uid(), kind: 'proto', path, protocol: protocol!, name: node?.name ?? baseName(path), draft: null, saved: 'null' };
        } else {
          const draft = await api.readRequest(p.root, path);
          tab = { id: uid(), kind: 'request', path, draft, saved: JSON.stringify(draft) };
        }
        // The project may have changed, or another caller loaded it meanwhile.
        if (get().project?.root !== p.root) return undefined;
        const again = docForParam(get().tabs, param);
        if (again) return again;
        set((s) => ({ tabs: [...s.tabs, tab] }));
        return tab;
      } catch (e) {
        toast.error(kind === 'folder' ? 'Could not open folder settings' : 'Could not open request', errorMessage(e));
        return undefined;
      } finally {
        loading.delete(key);
      }
    })();
    loading.set(key, job);
    return job;
  },

  async closeTab(id) {
    const t = get().tabs.find((x) => x.id === id);
    if (!t) return;
    const param = docParam(t);
    const views = Object.values(useWorkbench.getState().views).filter((v) => v.section === 'http' && v.param === param);
    for (const v of views) if (!(await useWorkbench.getState().close(v.id))) return;
  },

  updateRequest(tabId, fn) {
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === tabId && t.kind === 'request' ? { ...t, draft: fn(t.draft) } : t)) }));
    keepDocViews(get().tabs.find((t) => t.id === tabId));
    scheduleAutosave(tabId);
  },

  updateFolder(tabId, fn) {
    set((s) => ({ tabs: s.tabs.map((t) => (t.id === tabId && t.kind === 'folder' ? { ...t, draft: fn(t.draft) } : t)) }));
    keepDocViews(get().tabs.find((t) => t.id === tabId));
  },

  async save(tabId) {
    const t = get().tabs.find((x) => x.id === tabId);
    const p = get().project;
    if (!t || !p) return false;
    if (t.kind === 'proto') return true;
    try {
      if (t.kind === 'request') await api.saveRequest(p.root, t.path, t.draft);
      else await api.saveFolder(p.root, t.path, t.draft);
      const saved = JSON.stringify(t.draft);
      set((s) => ({ tabs: s.tabs.map((x) => (x.id === tabId ? { ...x, saved } : x)) }));
      warnOnCredentials(tabId, JSON.stringify(t.draft, null, 1));
      // Method/name changes show in the tree.
      if (t.kind === 'request') api.tree(p.root).then((tree) => set({ tree })).catch(() => {});
      return true;
    } catch (e) {
      toast.error('Save failed', errorMessage(e));
      return false;
    }
  },

  async rename(path, name) {
    const p = get().project!;
    // Flush unsaved edits first: the rename rewrites the file from disk.
    const open = get().tabs.find((t) => t.kind === 'request' && t.path === path);
    if (open && isDirty(open)) await get().save(open.id);
    try {
      const to = await api.renameItem(p.root, path, name);
      set((s) => ({
        tabs: repath(s.tabs, path, to).map((t) =>
          t.path !== to ? t : t.kind === 'request' ? { ...t, draft: { ...t.draft, name }, saved: JSON.stringify({ ...t.draft, name }) } : t.kind === 'proto' ? { ...t, name } : t,
        ),
      }));
      useWorkbench.getState().repathDocs('http', path, to);
      await get().refresh();
      return to;
    } catch (e) {
      toast.error('Rename failed', errorMessage(e));
    }
  },

  async remove(path) {
    const p = get().project!;
    try {
      await api.deleteItem(p.root, path);
      get().dropTabs([path]);
      await get().refresh();
      toast.success('Moved to the trash', path);
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  },

  async move(path, target, index) {
    const p = get().project!;
    try {
      const to = await api.moveItem(p.root, path, target, index);
      set((s) => ({ tabs: repath(s.tabs, path, to) }));
      useWorkbench.getState().repathDocs('http', path, to);
      await get().refresh();
    } catch (e) {
      toast.error('Move failed', errorMessage(e));
    }
  },

  async send(tabId) {
    const t = get().tabs.find((x) => x.id === tabId);
    const p = get().project;
    if (!t || t.kind !== 'request' || !p || t.sendId) return;
    const env = get().activeEnv();
    const method = t.draft.method.toUpperCase();
    if (env?.file.protected && !SAFE_METHODS.includes(method)) {
      const ok = await confirmDialog({
        title: `Send ${method} to ${env.file.name}?`,
        message: `"${env.file.name}" is a protected environment.`,
        confirmText: `Send ${method}`,
        danger: true,
      });
      if (!ok) return;
    }
    const sendId = uid();
    // A sent request keeps its tab: a preview replaced mid-send would cancel it.
    keepDocViews(t);
    set((s) => ({ tabs: s.tabs.map((x) => (x.id === tabId ? { ...x, sendId, stream: [], progress: 0 } : x)) }));
    // Stream messages arrive faster than React should render; batch them per animation frame.
    let pending: SseEvent[] = [];
    let bytes = 0;
    let scheduled = false;
    const flush = () => {
      scheduled = false;
      const add = pending;
      pending = [];
      set((s) => ({
        tabs: s.tabs.map((x) =>
          x.id === tabId && x.kind === 'request' ? { ...x, stream: [...(x.stream ?? []), ...add].slice(-MAX_STREAM_EVENTS), progress: bytes } : x,
        ),
      }));
    };
    const onEvent = (m: StreamMsg) => {
      if (m.type === 'sse') pending.push(m.event);
      else bytes = m.bytes;
      if (!scheduled) {
        scheduled = true;
        requestAnimationFrame(flush);
      }
    };
    try {
      const result = await api.send(sendId, p.root, t.path, t.draft, env?.id, onEvent);
      flush();
      set((s) => ({ tabs: s.tabs.map((x) => (x.id === tabId && x.kind === 'request' ? { ...x, result, sendId: undefined } : x)) }));
      for (const e of result.secretErrors) toast.error('Keychain error', e);
      if (result.captures.some((c) => c.value !== null)) useVarsVersion.getState().bump();
      useHistoryVersion.getState().bump();
    } catch (e) {
      set((s) => ({ tabs: s.tabs.map((x) => (x.id === tabId ? { ...x, sendId: undefined } : x)) }));
      toast.error('Send failed', errorMessage(e));
    }
  },

  dropTabs(paths) {
    const gone = (p: string) => paths.some((x) => p === x || p.startsWith(`${x}/`));
    const root = get().project?.root;
    for (const t of get().tabs) if (gone(t.path)) releaseDoc(t, root);
    set((s) => ({ tabs: s.tabs.filter((t) => !gone(t.path)) }));
    useWorkbench.getState().dropDocs('http', paths);
  },

  async cancel(tabId) {
    const t = get().tabs.find((x) => x.id === tabId);
    if (t?.kind === 'request' && t.sendId) await api.cancel(t.sendId);
  },
}));

/** Bumped when files change on disk outside the app (see modules/watch.rs). */
export const useFsVersion = create<{ v: number; paths: string[]; bump(paths: string[]): void }>((set) => ({
  v: 0,
  paths: [],
  bump: (paths) => set((s) => ({ v: s.v + 1, paths })),
}));

let unlistenFs: (() => void) | null = null;

async function watchProject(root: string) {
  try {
    await invoke('watch_project', { root });
    if (!unlistenFs) {
      unlistenFs = await listen<{ root: string; paths: string[] }>('project-changed', (e) => {
        const p = useWorkspace.getState().project;
        if (!p || e.payload.root !== p.root) return;
        useFsVersion.getState().bump(e.payload.paths);
        if (e.payload.paths.some((x) => x.startsWith('http/') || x.startsWith('environments/') || x === 'devall.yaml')) {
          useWorkspace.getState().refresh();
          useVarsVersion.getState().bump();
        }
      });
    }
  } catch {
    /* watching is best-effort; refresh-on-focus still works */
  }
}

async function loadProject(project: ProjectInfo) {
  watchProject(project.root);
  const tree = await api.tree(project.root);
  const prev = useWorkspace.getState();
  for (const t of prev.tabs) releaseDoc(t, prev.project?.root);
  useWorkspace.setState({ project, tree, tabs: [] });
  if (!project.isScratch) usePrefs.getState().set('lastProject', project.root);
  else usePrefs.getState().set('lastProject', null);
  // Default to the first environment the first time a project is opened.
  const envs = usePrefs.getState().activeEnv;
  if (!envs[project.root] && project.environments[0]) useWorkspace.getState().setActiveEnv(project.environments[0].id);
  // Tabs and splits are per project; documents load when their view shows.
  useWorkbench.getState().load(project.root, () => legacyTabs(project.root));
}

/** Request tabs saved before the workbench existed, as views (read once, when a project has no layout yet). */
function legacyTabs(root: string): ViewSpec[] | null {
  try {
    const saved = JSON.parse(localStorage.getItem(PROJECT_TABS_KEY(root)) ?? 'null') as { paths?: string[]; protos?: { path: string }[]; active?: string } | null;
    if (!saved) return null;
    const paths = [...(saved.paths ?? []), ...(saved.protos ?? []).map((p) => p.path)];
    if (!paths.length) return null;
    // The previously active one first so it is the active tab.
    const ordered = saved.active && paths.includes(saved.active) ? [saved.active, ...paths.filter((p) => p !== saved.active)] : paths;
    return ordered.map((param) => ({ section: 'http', param }));
  } catch {
    return null;
  }
}

/** Stops what an open document runs in the background (autosave timer, a send, a live connection). */
function releaseDoc(t: Tab, root: string | undefined) {
  clearTimeout(autosaveTimers.get(t.id));
  autosaveTimers.delete(t.id);
  if (t.kind === 'request' && t.sendId) api.cancel(t.sendId).catch(() => {});
  if (t.kind === 'proto' && root) closeLiveSession(root, t.path);
}

/** Preview tabs showing a document become permanent once it is edited or sent. */
function keepDocViews(t: Tab | undefined) {
  if (t) useWorkbench.getState().keepDoc('http', docParam(t));
}

/** Documents no view shows any more are released (unsaved ones stay until reopened or the project closes). */
useWorkbench.subscribe((s, prev) => {
  if (s.views === prev.views) return;
  const shown = new Set(Object.values(s.views).flatMap((v) => (v.section === 'http' && v.param ? [v.param] : [])));
  const { tabs, project } = useWorkspace.getState();
  const gone = tabs.filter((t) => !shown.has(docParam(t)) && !isDirty(t));
  if (!gone.length) return;
  for (const t of gone) releaseDoc(t, project?.root);
  useWorkspace.setState((st) => ({ tabs: st.tabs.filter((t) => !gone.includes(t)) }));
});

/** Closing the last view of a document with unsaved changes asks to save them. */
registerCloseGuard('http', async (view, layout) => {
  if (!view.param) return true;
  const others = Object.values(layout.views).some((v) => v.id !== view.id && v.section === 'http' && v.param === view.param);
  const ws = useWorkspace.getState();
  const t = docForParam(ws.tabs, view.param);
  if (others || !t || !isDirty(t)) return true;
  const name = t.kind === 'request' ? t.draft.name : t.kind === 'folder' ? (t.draft.name ?? (t.path || 'Project root')) : t.path;
  const choice = await saveChangesDialog({ title: 'Save changes?', message: `"${name}" has unsaved changes.` });
  if (choice === 'cancel') return false;
  if (choice === 'save') return ws.save(t.id);
  // Don't save: drop the edits so the document is released with its view.
  useWorkspace.setState((st) => ({ tabs: st.tabs.map((x) => (x.id === t.id ? ({ ...x, draft: JSON.parse(x.saved) } as Tab) : x)) }));
  return true;
});

export function findTreeNode(n: TreeNode | null, path: string): TreeNode | undefined {
  if (!n) return undefined;
  if (n.path === path) return n;
  if (n.kind === 'folder')
    for (const c of n.children) {
      const hit = findTreeNode(c, path);
      if (hit) return hit;
    }
  return undefined;
}

const baseName = (path: string) => path.split('/').pop()?.replace(/\.(req|ws|grpc|mqtt|sio)\.yaml$/, '') ?? path;

const lastFindings = new Map<string, string>();

/** Warns (once per distinct finding set) when a saved file contains something credential-shaped. */
export function warnOnCredentials(key: string, text: string) {
  scanApi
    .text(text)
    .then((findings) => {
      const sig = findings.map((f) => `${f.rule}:${f.excerpt}`).join('|');
      if (sig && sig !== lastFindings.get(key)) {
        const f = findings[0];
        toast.error(
          `Possible ${f.description.toLowerCase()} in a committed file`,
          `${f.excerpt} — store it as a secret and reference it as {{secret.NAME}}; secret values stay in your OS keychain.`,
        );
      }
      lastFindings.set(key, sig);
    })
    .catch(() => {});
}

function scheduleAutosave(tabId: string) {
  if (!usePrefs.getState().autoSave) return;
  clearTimeout(autosaveTimers.get(tabId));
  autosaveTimers.set(
    tabId,
    setTimeout(() => {
      autosaveTimers.delete(tabId);
      const t = useWorkspace.getState().tabs.find((x) => x.id === tabId);
      if (t && isDirty(t)) useWorkspace.getState().save(tabId);
    }, AUTOSAVE_MS),
  );
}
