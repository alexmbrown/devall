import { useCallback, useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { AlertTriangle, Database, DatabaseZap, Dices, FolderOpen, History, Plus, Trash2 } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { useContextMenu } from '@/components/ui/Menu';
import { copyLinkItems } from '@/components/links/LinkChip';
import { formatLink } from '@/lib/links/syntax';
import { Empty, Spinner } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { dataApi, type DataFileEntry } from '@/lib/api/data';
import { useNav, useViewLabel } from '@/store/nav';
import { useWorkspace } from '@/store/workspace';
import { FromDatabaseDialog } from './FromDatabaseDialog';
import { RandomizeDialog } from './RandomizeDialog';
import { SchemaEditor } from './SchemaEditor';
import './data.css';

const RANDOMIZE_EXAMPLE = `{
  "id": 42,
  "name": "Ada Lovelace",
  "email": "ada@example.com",
  "status": "active",
  "balance": 1250.5,
  "createdAt": "2024-05-01T10:20:30Z",
  "tags": ["admin", "beta"]
}
`;

/** Fake-data generator: schemas in `<project>/data/*.data.yaml`, editor, live preview, export. */
export default function DataPage() {
  const project = useWorkspace((s) => s.project);
  const go = useNav((s) => s.go);
  const param = useNav((s) => s.param);
  const [files, setFiles] = useState<DataFileEntry[] | null>(null);
  const [randomize, setRandomize] = useState(false);
  const [fromDb, setFromDb] = useState(false);
  const dirty = useRef(false);
  const ctx = useContextMenu();
  const root = project?.root;

  const load = useCallback(async () => {
    if (!root) return;
    try {
      setFiles(await dataApi.list(root));
    } catch (e) {
      toast.error('Could not list data schemas', errorMessage(e));
      setFiles([]);
    }
  }, [root]);

  useEffect(() => {
    setFiles(null);
    void load();
  }, [load]);

  // Re-read the list when the window regains focus (files edited outside the app).
  useEffect(() => {
    const onFocus = () => void load();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [load]);

  const selected = param && files?.some((f) => f.path === param) ? param : files?.find((f) => !f.error)?.path;
  useViewLabel(files ? (files.find((f) => f.path === selected)?.name ?? null) : undefined);
  const onDirtyChange = useCallback((d: boolean) => void (dirty.current = d), []);

  const select = async (path: string) => {
    if (path === selected) return;
    if (dirty.current) {
      const ok = await confirmDialog({ title: 'Discard unsaved changes?', message: 'The current schema has changes that are not saved.', confirmText: 'Discard', danger: true });
      if (!ok) return;
    }
    dirty.current = false;
    go('fakedata', path);
  };

  const create = async () => {
    if (!root) return;
    const name = await promptDialog({ title: 'New data schema', label: 'Name', placeholder: 'Users and orders', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    try {
      const path = await dataApi.create(root, name);
      await load();
      dirty.current = false;
      go('fakedata', path);
    } catch (e) {
      toast.error('Could not create schema', errorMessage(e));
    }
  };

  const remove = async (f: DataFileEntry) => {
    if (!root) return;
    const ok = await confirmDialog({ title: `Delete “${f.name}”?`, message: `data/${f.path} moves to your system trash.`, confirmText: 'Move to trash', danger: true });
    if (!ok) return;
    try {
      await dataApi.remove(root, f.path);
      if (f.path === selected) {
        dirty.current = false;
        go('fakedata');
      }
      await load();
    } catch (e) {
      toast.error('Delete failed', errorMessage(e));
    }
  };

  if (!project)
    return (
      <>
        <Empty
          icon={<Database />}
          title="Fake data needs a project"
          action={
            <div className="row-sm">
              <button className="btn btn-sm btn-primary" onClick={() => go('projects')}>
                Open a project
              </button>
              <button className="btn btn-sm" onClick={() => setRandomize(true)}>
                <Dices /> Randomize a JSON body…
              </button>
            </div>
          }
        >
          Generator schemas are saved in the project's <code className="code-inline">data/</code> folder so they can be committed and shared.
        </Empty>
        <RandomizeDialog open={randomize} onClose={() => setRandomize(false)} body={RANDOMIZE_EXAMPLE} />
      </>
    );

  return (
    <div className="split">
      <SidePane
        title="Data schemas"
        actions={
          <>
            <button className="icon-btn" onClick={() => setRandomize(true)} title="Randomize a JSON response body (WireMock template)" aria-label="Randomize a JSON body">
              <Dices />
            </button>
            <button className="icon-btn" onClick={() => setFromDb(true)} title="New from database… (tables, keys and enums of a connected database)" aria-label="New from database">
              <DatabaseZap />
            </button>
            <button className="icon-btn" onClick={() => void create()} title="New schema" aria-label="New schema">
              <Plus />
            </button>
          </>
        }
      >
        <div className="list">
          {files === null ? (
            <div className="empty">
              <Spinner />
            </div>
          ) : files.length === 0 ? (
            <Empty
              title="No schemas yet"
              action={
                <div className="row-sm">
                  <button className="btn btn-sm btn-primary" onClick={() => void create()}>
                    <Plus /> New schema
                  </button>
                  <button className="btn btn-sm" onClick={() => setFromDb(true)}>
                    <DatabaseZap /> New from database…
                  </button>
                </div>
              }
            >
              Seeded, relational test data — JSON, CSV, SQL or YAML. Free Mockaroo, on your machine.
            </Empty>
          ) : (
            files.map((f) => (
              <div
                key={f.path}
                className={clsx('list-item', f.path === selected && 'on')}
                onClick={() => void select(f.path)}
                onContextMenu={(e) =>
                  ctx.open(e, [
                    { label: 'Reveal file', icon: <FolderOpen />, onSelect: () => void revealItemInDir(`${project.root}/data/${f.path}`).catch((err) => toast.error('Could not reveal', errorMessage(err))) },
                    { label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `data/${f.path}`, title: f.name }) },
                    ...copyLinkItems(formatLink('fakedata', f.path), f.name),
                    { separator: true },
                    { label: 'Delete', icon: <Trash2 />, danger: true, onSelect: () => void remove(f) },
                  ])
                }
                title={f.error ?? `data/${f.path}`}
              >
                {f.error ? <AlertTriangle size={14} className="text-warning" /> : <Database size={14} className="subtle" />}
                <div className="grow" style={{ minWidth: 0 }}>
                  <div className="truncate">{f.name}</div>
                  <div className="meta truncate">{f.error ? 'cannot be parsed' : f.entities.join(', ') || 'no entities'}</div>
                </div>
              </div>
            ))
          )}
        </div>
      </SidePane>
      <div className="pane fill">
        {selected ? (
          <SchemaEditor
            key={`${project.root}|${selected}`}
            root={project.root}
            path={selected}
            onSaved={() => void load()}
            onDeleted={() => {
              dirty.current = false;
              go('fakedata');
              void load();
            }}
            onDirtyChange={onDirtyChange}
          />
        ) : files?.some((f) => f.error) ? (
          <Empty icon={<AlertTriangle />} title="Schemas need fixing">
            {files.find((f) => f.error)?.error}
          </Empty>
        ) : (
          <Empty icon={<Database />} title="No schema selected">
            Create a schema, then add entities and fields — or infer them from a JSON sample or an OpenAPI document.
          </Empty>
        )}
      </div>
      {ctx.node}
      <RandomizeDialog open={randomize} onClose={() => setRandomize(false)} body={RANDOMIZE_EXAMPLE} />
      <FromDatabaseDialog
        open={fromDb}
        root={project.root}
        onClose={() => setFromDb(false)}
        onCreated={(path) => {
          dirty.current = false;
          void load().then(() => go('fakedata', path));
        }}
      />
    </div>
  );
}
