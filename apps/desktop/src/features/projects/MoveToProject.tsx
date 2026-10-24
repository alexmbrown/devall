import { useEffect, useMemo, useState } from 'react';
import { create } from 'zustand';
import { ArrowRightLeft, FileText, Folder, FolderOpen, FolderPlus, GitBranch, Layers } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, MethodBadge, Spinner, Switch } from '@/components/ui/misc';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { notesApi, type NoteNode } from '@/lib/api/notes';
import type { ProjectInfo, RecentProject, TransferResult, TreeNode } from '@/lib/types';
import { useNav } from '@/store/nav';
import { isDirty, useWorkspace } from '@/store/workspace';

interface MoveState {
  open: boolean;
  /** Paths under http/ preselected (from the request tree); empty = everything. */
  items: string[];
  show(items?: string[]): void;
  close(): void;
}

/** “Move to project…”: copies (or moves) requests, environments and notes into another project. */
export const useMoveToProject = create<MoveState>((set) => ({
  open: false,
  items: [],
  show: (items = []) => set({ open: true, items }),
  close: () => set({ open: false }),
}));

interface Destination {
  root: string;
  name: string;
}

function findNode(node: TreeNode | null, path: string): TreeNode | null {
  if (!node) return null;
  if (node.path === path) return node;
  if (node.kind === 'folder')
    for (const c of node.children) {
      const hit = findNode(c, path);
      if (hit) return hit;
    }
  return null;
}

export function MoveToProjectDialog() {
  const { open, items: preselected, close } = useMoveToProject();
  const project = useWorkspace((s) => s.project);
  if (!open || !project) return null;
  return <MoveDialog key={`${project.root}|${preselected.join(',')}`} project={project} preselected={preselected} onClose={close} />;
}

function MoveDialog({ project, preselected, onClose }: { project: ProjectInfo; preselected: string[]; onClose(): void }) {
  const tree = useWorkspace((s) => s.tree);
  const wholeProject = preselected.length === 0;
  const [recent, setRecent] = useState<RecentProject[] | null>(null);
  const [dest, setDest] = useState<Destination | null>(null);
  const [notes, setNotes] = useState<NoteNode[]>([]);
  // Request tree entries offered: the root's children plus anything preselected deeper down.
  const entries = useMemo(() => {
    const top = tree?.kind === 'folder' ? tree.children : [];
    const extra = preselected.map((p) => findNode(tree, p)).filter((n): n is TreeNode => !!n && !top.some((t) => t.path === n.path));
    return [...extra, ...top];
  }, [tree, preselected]);
  const [items, setItems] = useState<Set<string>>(() => new Set(wholeProject ? entries.map((e) => e.path) : preselected));
  const [envs, setEnvs] = useState<Set<string>>(() => new Set(wholeProject ? project.environments.map((e) => e.id) : []));
  const [noteSel, setNoteSel] = useState<Set<string>>(new Set());
  const [keepFolders, setKeepFolders] = useState(true);
  const [includeVariables, setIncludeVariables] = useState(true);
  const [removeSource, setRemoveSource] = useState(project.isScratch);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .recentProjects()
      .then((r) => setRecent(r.filter((x) => x.path !== project.root)))
      .catch(() => setRecent([]));
    notesApi
      .tree(project.root)
      .then((t) => {
        const list = t.kind === 'folder' ? t.children : [];
        setNotes(list);
        if (wholeProject) setNoteSel(new Set(list.map((n) => n.path)));
      })
      .catch(() => setNotes([]));
  }, [project.root, wholeProject]);

  const toggle = (set: Set<string>, setter: (s: Set<string>) => void, key: string, on: boolean) => {
    const next = new Set(set);
    if (on) next.add(key);
    else next.delete(key);
    setter(next);
  };

  const browse = async () => {
    const dir = await openDialog({ directory: true, title: 'Choose the destination project (folder with devall.yaml)' });
    if (typeof dir !== 'string') return;
    try {
      const info = await api.openProject(dir);
      if (info.root === project.root) return toast.info('That is the current project');
      setDest({ root: info.root, name: info.manifest.name });
    } catch (e) {
      toast.error('Not a /dev/all project', errorMessage(e));
    }
  };

  const createNew = async () => {
    const dir = await openDialog({ directory: true, title: 'Choose a folder for the new project (e.g. your service’s git repo)' });
    if (typeof dir !== 'string') return;
    const suggested = dir.split(/[\\/]/).filter(Boolean).pop() ?? 'My project';
    const name = await promptDialog({ title: 'New project', label: 'Project name', initial: suggested, validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    try {
      const info = await api.createProject(dir, name);
      setDest({ root: info.root, name: info.manifest.name });
    } catch (e) {
      toast.error('Could not create the project', errorMessage(e));
    }
  };

  const nothing = items.size === 0 && envs.size === 0 && noteSel.size === 0;

  const run = async () => {
    if (!dest || nothing) return;
    setBusy(true);
    const ws = useWorkspace.getState();
    try {
      // Unsaved edits travel too: flush the tabs of the selected items first.
      const picked = [...items];
      for (const t of ws.tabs) if (isDirty(t) && picked.some((p) => t.path === p || t.path.startsWith(`${p}/`))) await ws.save(t.id);
      const result: TransferResult = await api.transferToProject(
        project.root,
        dest.root,
        { items: picked, environments: [...envs], notes: [...noteSel] },
        { keepFolders, includeVariables, removeSource },
      );
      const r = result.report;
      if (r.removedSource) ws.dropTabs(picked);
      await ws.refresh();
      onClose();
      await offerSecrets(project, dest, result);
      for (const w of r.warnings) toast.info('Check after moving', w);
      const what = [r.requests && `${r.requests} request${r.requests === 1 ? '' : 's'}`, r.environments.length && `${r.environments.length} environment${r.environments.length === 1 ? '' : 's'}`, r.notes.length && `${r.notes.length} note${r.notes.length === 1 ? '' : 's'}`]
        .filter(Boolean)
        .join(', ');
      toast.success(`${r.removedSource ? 'Moved' : 'Copied'} to ${dest.name}`, what || 'Nothing to copy', {
        label: 'Open project',
        run: async () => {
          if (await useWorkspace.getState().openProject(dest.root)) {
            if (r.firstRequest) await useWorkspace.getState().openRequest(r.firstRequest);
            else useNav.getState().go('http');
          }
        },
      });
    } catch (e) {
      toast.error('Could not move to the project', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const verb = removeSource ? 'Move' : 'Copy';
  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={
        <>
          <ArrowRightLeft size={16} /> {project.isScratch ? 'Move from the scratch pad to a project' : 'Copy to another project'}
        </>
      }
      footer={
        <>
          <span className="subtle xs grow">Name clashes get “(2)”; nothing in the destination is overwritten.</span>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!dest || nothing || busy} onClick={run}>
            {busy ? <Spinner /> : <ArrowRightLeft />} {verb} {dest ? `to ${dest.name}` : ''}
          </button>
        </>
      }
    >
      <div className="move-grid">
        <section className="col">
          <span className="label">Destination</span>
          <div className="panel move-list">
            {recent === null ? (
              <div className="p-4">
                <Spinner />
              </div>
            ) : recent.length === 0 ? (
              <p className="subtle xs p-4">No recent projects — open or create one.</p>
            ) : (
              recent.map((r) => (
                <div key={r.path} className={`list-item${dest?.root === r.path ? ' on' : ''}`} onClick={() => setDest({ root: r.path, name: r.name })} title={r.path}>
                  <GitBranch size={14} className="subtle" />
                  <span className="strong truncate">{r.name}</span>
                  <span className="subtle xs mono truncate grow">{r.path}</span>
                </div>
              ))
            )}
            {dest && !recent?.some((r) => r.path === dest.root) && (
              <div className="list-item on" title={dest.root}>
                <GitBranch size={14} className="subtle" />
                <span className="strong truncate">{dest.name}</span>
                <span className="subtle xs mono truncate grow">{dest.root}</span>
              </div>
            )}
          </div>
          <div className="row-sm">
            <button className="btn btn-sm" onClick={browse}>
              <FolderOpen /> Other project…
            </button>
            <button className="btn btn-sm" onClick={createNew}>
              <FolderPlus /> New project…
            </button>
          </div>
          <span className="label mt-4">Options</span>
          <Switch checked={keepFolders} onChange={setKeepFolders} label="Recreate parent folders (keeps inherited headers and auth)" />
          <Switch checked={includeVariables} onChange={setIncludeVariables} label="Copy project variables the destination doesn’t have" />
          <Switch checked={removeSource} onChange={setRemoveSource} label={`Remove from ${project.isScratch ? 'the scratch pad' : 'this project'} afterwards (to the trash)`} />
        </section>
        <section className="col">
          <span className="label">Requests and folders</span>
          <div className="panel move-list">
            {entries.length === 0 && <p className="subtle xs p-4">No requests.</p>}
            {entries.map((n) => (
              <div key={n.path} className="list-item" onClick={() => toggle(items, setItems, n.path, !items.has(n.path))}>
                <Checkbox checked={items.has(n.path)} onChange={(v) => toggle(items, setItems, n.path, v)} />
                {n.kind === 'folder' ? <Folder size={13} className="subtle" /> : <MethodBadge method={n.method} short />}
                <span className="truncate grow">{n.name}</span>
                <span className="subtle xs mono truncate">{n.path}</span>
              </div>
            ))}
          </div>
          {project.environments.length > 0 && (
            <>
              <span className="label">Environments</span>
              <div className="panel move-list">
                {project.environments.map((e) => (
                  <div key={e.id} className="list-item" onClick={() => toggle(envs, setEnvs, e.id, !envs.has(e.id))}>
                    <Checkbox checked={envs.has(e.id)} onChange={(v) => toggle(envs, setEnvs, e.id, v)} />
                    <Layers size={13} className="subtle" />
                    <span className="truncate grow">{e.file.name}</span>
                    {(e.file.secrets?.length ?? 0) > 0 && <span className="subtle xs">{e.file.secrets!.length} secret name(s)</span>}
                  </div>
                ))}
              </div>
            </>
          )}
          {notes.length > 0 && (
            <>
              <span className="label">Notes</span>
              <div className="panel move-list">
                {notes.map((n) => (
                  <div key={n.path} className="list-item" onClick={() => toggle(noteSel, setNoteSel, n.path, !noteSel.has(n.path))}>
                    <Checkbox checked={noteSel.has(n.path)} onChange={(v) => toggle(noteSel, setNoteSel, n.path, v)} />
                    {n.kind === 'folder' ? <Folder size={13} className="subtle" /> : <FileText size={13} className="subtle" />}
                    <span className="truncate grow">{n.kind === 'folder' ? n.name : n.title}</span>
                  </div>
                ))}
              </div>
            </>
          )}
          <p className="subtle xs">Secret values stay in your OS keychain; afterwards you can choose to copy them into the destination’s keychain scope.</p>
        </section>
      </div>
      {!dest && <Callout kind="info">Pick the project to move into.</Callout>}
    </Modal>
  );
}

/** Asks before copying keychain values (only names are shown — values never reach the UI). */
async function offerSecrets(project: ProjectInfo, dest: Destination, result: TransferResult) {
  if (!result.secrets.length) return;
  const label = (s: TransferResult['secrets'][number]) => `${s.name}${s.fromEnv === '*' ? '' : ` (${s.toEnv})`}${s.exists ? ' — replaces the existing value' : ''}`;
  const ok = await confirmDialog({
    title: `Copy ${result.secrets.length} secret value${result.secrets.length === 1 ? '' : 's'} to ${dest.name}?`,
    message: `${project.isScratch ? 'The scratch pad' : project.manifest.name} has keychain values for: ${result.secrets.map(label).join(', ')}. Copy them into ${dest.name}’s keychain scope so the moved requests work there? They are never written to project files.`,
    confirmText: 'Copy secrets',
  });
  if (!ok) return;
  try {
    const n = await api.transferSecrets(project.root, dest.root, result.secrets);
    toast.success(`Copied ${n} secret value${n === 1 ? '' : 's'}`, 'Stored in the OS keychain for the destination project.');
  } catch (e) {
    toast.error('Could not copy the secrets', errorMessage(e));
  }
}
