import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { AlertTriangle, ArrowRightLeft, CheckSquare, ChevronRight, Columns2, ClipboardPaste, CopyPlus, FileCheck2, FileDown, FileUp, Folder, FolderOpen, FolderPlus, History, Link2, NotebookPen, Pencil, Play, Plus, Radio, Settings2, Trash2, X } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { useOpenApi } from '@/features/openapi';
import { AiAssistButton } from '@/features/ai';
import { useImportExport } from '@/features/import';
import { PROTO_NEW_ITEMS, newProtoRequest } from '@/features/proto';
import { useRunner } from './RunnerModal';
import { useMoveToProject } from '@/features/projects/MoveToProject';
import { openQuickAdd } from '@/features/todos/QuickAddDialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { copyLinkItems } from '@/components/links/LinkChip';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { MethodBadge, SearchInput } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { api, errorMessage } from '@/lib/ipc';
import { formatLink } from '@/lib/links/syntax';
import type { TreeNode } from '@/lib/types';
import { useNav } from '@/store/nav';
import { docParam, useWorkspace } from '@/store/workspace';

const OPEN_KEY = 'devall.treeOpen';

function loadOpen(): Record<string, boolean> {
  try {
    return JSON.parse(localStorage.getItem(OPEN_KEY) ?? '{}');
  } catch {
    return {};
  }
}

function filterTree(node: TreeNode, q: string): TreeNode | null {
  if (node.kind === 'request') return `${node.name} ${node.method} ${node.path}`.toLowerCase().includes(q) ? node : null;
  const children = node.children.map((c) => filterTree(c, q)).filter(Boolean) as TreeNode[];
  return children.length || (node.path && node.name.toLowerCase().includes(q)) ? { ...node, children } : null;
}

export function RequestTree() {
  const tree = useWorkspace((s) => s.tree);
  const project = useWorkspace((s) => s.project!);
  // Inside an `http` view: highlight and navigate that view's document.
  const activePath = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const openNode = (node: TreeNode, keep = false) => go('http', node.path, keep ? { preview: false } : undefined);
  const openFolderSettings = (path: string) => go('http', docParam({ kind: 'folder', path }), { preview: false });
  const ws = useWorkspace.getState;
  const [q, setQ] = useState('');
  const [open, setOpenState] = useState<Record<string, boolean>>(loadOpen);
  const [drag, setDrag] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const ctx = useContextMenu();
  const [scratchHintHidden, setScratchHintHidden] = useState(false);

  const setOpen = (path: string, v: boolean) => {
    const next = { ...open, [`${project.root}|${path}`]: v };
    setOpenState(next);
    try {
      localStorage.setItem(OPEN_KEY, JSON.stringify(next));
    } catch {
      /* ignore */
    }
  };
  const isOpen = (path: string) => q !== '' || open[`${project.root}|${path}`] !== false;

  const shown = useMemo(() => (tree && q ? filterTree(tree, q.toLowerCase()) : tree), [tree, q]);

  const newFolder = async (parent: string) => {
    const name = await promptDialog({ title: 'New folder', label: 'Name', placeholder: 'Users', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    try {
      const p = await api.createFolder(project.root, parent, name);
      setOpen(p, true);
      await ws().refresh();
    } catch (e) {
      toast.error('Could not create folder', errorMessage(e));
    }
  };

  const importCurl = async (folder: string) => {
    const cmd = await promptDialog({ title: 'Import cURL', label: 'Paste a cURL command (bash, cmd or PowerShell style)', placeholder: "curl 'https://api.example.com/users' -H 'Accept: application/json'" });
    if (!cmd) return;
    try {
      const req = await api.importCurl(cmd);
      await ws().newRequest(folder, req);
    } catch (e) {
      toast.error('Could not parse cURL', errorMessage(e));
    }
  };

  const renameItem = async (node: TreeNode) => {
    const name = await promptDialog({ title: `Rename ${node.kind}`, label: 'Name', initial: node.name, validate: (v) => (v.trim() ? undefined : 'Required') });
    if (name && name !== node.name) await ws().rename(node.path, name);
  };

  const deleteItem = async (node: TreeNode) => {
    const ok = await confirmDialog({
      title: `Delete ${node.kind === 'folder' ? 'folder' : 'request'} “${node.name}”?`,
      message: node.kind === 'folder' ? 'The folder and everything in it move to your system trash.' : 'The file moves to your system trash, so you can restore it.',
      confirmText: 'Move to trash',
      danger: true,
    });
    if (ok) await ws().remove(node.path);
  };

  const folderMenu = (path: string): MenuItemDef[] => [
    { label: 'New request', icon: <Plus />, onSelect: () => ws().newRequest(path) },
    ...PROTO_NEW_ITEMS.map(({ kind, label }) => ({
      label,
      icon: <Radio />,
      onSelect: async () => {
        const p = await newProtoRequest(kind, path);
        if (p) ws().openProto(p, kind, label.replace(/^New /, ''), { preview: false });
      },
    })),
    { label: 'New folder', icon: <FolderPlus />, onSelect: () => newFolder(path) },
    { label: 'Import cURL…', icon: <ClipboardPaste />, onSelect: () => importCurl(path) },
    { label: 'Import collection or spec…', icon: <FileUp />, onSelect: () => useImportExport.getState().openImport(path) },
    { label: path ? 'Export folder…' : 'Export project…', icon: <FileDown />, onSelect: () => useImportExport.getState().openExport(path) },
    { label: path ? 'Run folder…' : 'Run all requests…', icon: <Play />, onSelect: () => useRunner.getState().open(path) },
    ...(path ? [{ label: 'OpenAPI spec: link & re-sync…', icon: <Link2 />, onSelect: () => useOpenApi.getState().open({ tab: 'link', folder: path }) }] : []),
    { label: 'OpenAPI: lint / check drift…', icon: <FileCheck2 />, onSelect: () => useOpenApi.getState().open({ tab: 'lint', folder: path || undefined }) },
  ];

  // Personal to-do linked to this item; quick-add syntax works (tomorrow, p1, @label, #list).
  const todoAbout = (node: TreeNode) => () => openQuickAdd({ root: project.root, title: `Add to-do about “${node.name}”`, defaults: { links: [formatLink(node.kind === 'folder' ? 'folder' : 'request', node.path)] } });

  const itemMenu = (node: TreeNode): MenuItemDef[] => [
    ...(node.kind === 'folder'
      ? [
          ...folderMenu(node.path),
          { separator: true },
          { label: 'Folder settings (headers, auth, variables)', icon: <Settings2 />, onSelect: () => openFolderSettings(node.path) },
          { label: 'Folder settings history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `http/${node.path}/_folder.yaml`, title: `${node.name} (folder settings)` }) },
        ]
      : [
          {
            label: 'Duplicate',
            icon: <CopyPlus />,
            onSelect: async () => {
              const p = await api.duplicateRequest(project.root, node.path);
              await ws().refresh();
              await ws().openRequest(p, { preview: false });
            },
          },
          { label: 'Open in new tab', icon: <Plus />, onSelect: () => go('http', node.path, { newTab: true }) },
          { label: 'Open to the side', icon: <Columns2 />, onSelect: () => go('http', node.path, { toSide: true }) },
          { label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `http/${node.path}`, title: node.name }) },
        ]),
    { label: 'Rename…', icon: <Pencil />, onSelect: () => renameItem(node) },
    { label: 'Add to-do about this…', icon: <CheckSquare />, onSelect: todoAbout(node) },
    ...copyLinkItems(formatLink(node.kind === 'folder' ? 'folder' : 'request', node.path), node.name),
    { label: project.isScratch ? 'Move to project…' : 'Copy to another project…', icon: <ArrowRightLeft />, onSelect: () => useMoveToProject.getState().show([node.path]) },
    {
      label: 'Reveal in file manager',
      icon: <FolderOpen />,
      onSelect: () => revealItemInDir(`${project.root}/http/${node.path}`).catch((e) => toast.error('Could not reveal', String(e))),
    },
    { separator: true },
    { label: 'Move to trash', icon: <Trash2 />, danger: true, onSelect: () => deleteItem(node) },
  ];

  const onDrop = async (target: TreeNode, e: React.DragEvent) => {
    e.preventDefault();
    setDropTarget(null);
    const src = drag;
    setDrag(null);
    if (!src || src === target.path) return;
    if (target.kind === 'folder') await ws().move(src, target.path);
    else {
      // Dropping on a request places the item right before it, in that request's folder.
      const parent = target.path.includes('/') ? target.path.slice(0, target.path.lastIndexOf('/')) : '';
      const siblings = findChildren(tree!, parent);
      const index = siblings.findIndex((c) => c.path === target.path);
      await ws().move(src, parent, index < 0 ? undefined : index);
    }
  };

  const render = (node: TreeNode, depth: number): React.ReactNode => {
    const pad = { paddingLeft: 8 + depth * 14 };
    const dragProps = {
      draggable: true,
      onDragStart: (e: React.DragEvent) => {
        setDrag(node.path);
        e.dataTransfer.effectAllowed = 'move';
      },
      onDragOver: (e: React.DragEvent) => {
        if (drag && drag !== node.path) {
          e.preventDefault();
          setDropTarget(node.path);
        }
      },
      onDragLeave: () => setDropTarget((t) => (t === node.path ? null : t)),
      onDrop: (e: React.DragEvent) => onDrop(node, e),
      onDragEnd: () => {
        setDrag(null);
        setDropTarget(null);
      },
    };
    if (node.kind === 'folder') {
      const o = isOpen(node.path);
      return (
        <div key={node.path}>
          <div
            className={clsx('tree-row folder', dropTarget === node.path && 'drop')}
            style={pad}
            onClick={() => setOpen(node.path, !o)}
            onContextMenu={(e) => ctx.open(e, itemMenu(node))}
            onDoubleClick={() => openFolderSettings(node.path)}
            {...dragProps}
          >
            <ChevronRight className={clsx('chev', o && 'open')} />
            {o ? <FolderOpen className="folder-icon" /> : <Folder className="folder-icon" />}
            <span className="truncate">{node.name}</span>
            <span className="count">{countRequests(node)}</span>
          </div>
          {o && node.children.map((c) => render(c, depth + 1))}
        </div>
      );
    }
    return (
      <div
        key={node.path}
        className={clsx('tree-row', activePath === node.path && 'on', dropTarget === node.path && 'drop-before', node.error && 'invalid')}
        style={pad}
        title={node.error ?? node.path}
        onClick={(e) => (e.ctrlKey || e.metaKey ? go('http', node.path, { newTab: true }) : openNode(node))}
        onDoubleClick={() => openNode(node, true)}
        onAuxClick={(e) => e.button === 1 && go('http', node.path, { newTab: true })}
        onContextMenu={(e) => ctx.open(e, itemMenu(node))}
        {...dragProps}
      >
        {node.error ? <AlertTriangle className="text-danger" size={13} /> : <MethodBadge method={node.method} short />}
        <span className="truncate">{node.name}</span>
      </div>
    );
  };

  return (
    <SidePane
      title="Requests"
      actions={
        <>
          <AiAssistButton feature="nlRequest" compact align="right" title="Describe a request in plain words" input={(d) => ({ description: d })} onResult={(r) => ws().newRequest('', r.request)} applyLabel="Create request" />
          <Menu
            align="right"
            items={folderMenu('')}
            trigger={(p) => (
              <button className="icon-btn" {...p} title="New…" aria-label="New">
                <Plus />
              </button>
            )}
          />
        </>
      }
      toolbar={<SearchInput value={q} onChange={setQ} placeholder="Filter requests" className="grow" />}
    >
      {project.isScratch && !scratchHintHidden && tree?.kind === 'folder' && tree.children.length > 0 && (
        <div className="scratch-banner">
          <NotebookPen size={14} />
          <span className="small grow">Scratch pad — keep what matters in a project.</span>
          <button className="btn btn-sm" onClick={() => useMoveToProject.getState().show()}>
            <ArrowRightLeft /> Move to project…
          </button>
          <button className="icon-btn sm" aria-label="Hide" title="Hide for this session" onClick={() => setScratchHintHidden(true)}>
            <X />
          </button>
        </div>
      )}
      <div
        className={clsx('tree', dropTarget === '' && 'drop')}
        onContextMenu={(e) => ctx.open(e, folderMenu(''))}
        onDragOver={(e) => {
          if (drag && e.target === e.currentTarget) {
            e.preventDefault();
            setDropTarget('');
          }
        }}
        onDrop={(e) => {
          if (e.target !== e.currentTarget || !drag) return;
          e.preventDefault();
          const src = drag;
          setDrag(null);
          setDropTarget(null);
          ws().move(src, '');
        }}
      >
        {shown && shown.kind === 'folder' && shown.children.length > 0 ? (
          shown.children.map((c) => render(c, 0))
        ) : (
          <div className="empty small">
            {q ? (
              <p>No requests match “{q}”.</p>
            ) : (
              <>
                <p>No requests yet.</p>
                <div className="row-sm">
                  <button className="btn btn-sm btn-primary" onClick={() => ws().newRequest('')}>
                    <Plus /> New request
                  </button>
                  <button className="btn btn-sm" onClick={() => importCurl('')}>
                    <ClipboardPaste /> Import cURL
                  </button>
                  <button className="btn btn-sm" onClick={() => useImportExport.getState().openImport('')} title="Postman, Insomnia, Bruno, OpenAPI, HAR, .http, Hurl">
                    <FileUp /> Import collection
                  </button>
                </div>
              </>
            )}
          </div>
        )}
      </div>
      {ctx.node}
    </SidePane>
  );
}

function countRequests(n: TreeNode): number {
  return n.kind === 'request' ? 1 : n.children.reduce((a, c) => a + countRequests(c), 0);
}

function findChildren(root: TreeNode, path: string): TreeNode[] {
  if (root.kind !== 'folder') return [];
  if (root.path === path) return root.children;
  for (const c of root.children) {
    const r = findChildren(c, path);
    if (r.length || (c.kind === 'folder' && c.path === path)) return r;
  }
  return [];
}
