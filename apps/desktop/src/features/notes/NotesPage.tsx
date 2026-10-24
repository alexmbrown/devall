import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { BookOpenCheck, ChevronRight, FilePlus, FolderOpen, FolderPlus, Folder, History, Link2, NotebookText, Pencil, Trash2 } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { Markdown } from '@/components/Markdown';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { Empty, SearchInput, Segmented } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { notesApi, type NoteHit, type NoteNode } from '@/lib/api/notes';
import { notebooksApi } from '@/lib/api/notebooks';
import { NotebookEditor, createNotebook, useRunnableCodeBlocks } from '@/features/notebooks';
import { AiAssistButton } from '@/features/ai';
import { openStubFile } from '@/features/mocks';
import { resolveProjectLink } from './links';
import type { TreeNode } from '@/lib/types';
import { useKeepView, useNav } from '@/store/nav';
import { useFsVersion, useWorkspace } from '@/store/workspace';

type Mode = 'edit' | 'split' | 'preview';
const SAVE_MS = 700;

function flattenRequests(n: TreeNode | null, out: { path: string; name: string; method: string }[] = []) {
  if (!n) return out;
  if (n.kind === 'request') out.push({ path: n.path, name: n.name, method: n.method });
  else n.children.forEach((c) => flattenRequests(c, out));
  return out;
}

/**
 * Opens project links from rendered Markdown: requests (`http/…`), notes (also relative to the
 * current note: `other.md`, `../ops/runbook.md`), mock stubs (`mocks/mappings/…`), webhook inboxes,
 * load tests, connections, saved queries, fake-data schemas and environments.
 */
export function useProjectLinkOpener() {
  const go = useNav((s) => s.go);
  return useCallback(
    (href: string, fromNote?: string) => {
      const t = resolveProjectLink(href, fromNote);
      switch (t.kind) {
        case 'request':
          void useWorkspace.getState().openRequest(t.path);
          break;
        case 'stub':
          openStubFile(t.file);
          break;
        case 'section':
          go(t.section, t.param);
          break;
        default:
          toast.info('Not a link to something in this project', href);
      }
    },
    [go],
  );
}

export default function NotesPage() {
  const project = useWorkspace((s) => s.project!);
  const requestTree = useWorkspace((s) => s.tree);
  const param = useNav((s) => s.param);
  const go = useNav((s) => s.go);
  const [tree, setTree] = useState<NoteNode | null>(null);
  const [q, setQ] = useState('');
  const [hits, setHits] = useState<NoteHit[]>([]);
  const [closed, setClosed] = useState<Record<string, boolean>>({});
  const ctx = useContextMenu();
  const selected = param && (param.endsWith('.md') || param.endsWith('.ipynb')) ? param : null;
  const fsVersion = useFsVersion((s) => s.v);

  const reload = useCallback(() => notesApi.tree(project.root).then(setTree).catch((e) => toast.error('Could not read notes', errorMessage(e))), [project.root]);
  useEffect(() => {
    reload();
  }, [reload, fsVersion]);
  useEffect(() => {
    if (!q.trim()) return setHits([]);
    const t = setTimeout(() => notesApi.search(project.root, q).then(setHits).catch(() => setHits([])), 150);
    return () => clearTimeout(t);
  }, [q, project.root]);

  const newNote = async (folder: string) => {
    const title = await promptDialog({ title: 'New note', label: 'Title', placeholder: 'Architecture decisions', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!title) return;
    try {
      const path = await notesApi.create(project.root, folder, title);
      await reload();
      go('notes', path);
    } catch (e) {
      toast.error('Could not create note', errorMessage(e));
    }
  };
  const newNotebook = async (folder: string) => {
    const title = await promptDialog({ title: 'New notebook', label: 'Title', placeholder: 'Run the app locally', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!title) return;
    try {
      const path = await createNotebook(project.root, folder, title);
      await reload();
      go('notes', path);
    } catch (e) {
      toast.error('Could not create notebook', errorMessage(e));
    }
  };
  const newFolder = async (parent: string) => {
    const name = await promptDialog({ title: 'New folder', label: 'Name', validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name) return;
    await notesApi.createFolder(project.root, parent, name).catch((e) => toast.error('Could not create folder', errorMessage(e)));
    reload();
  };
  const rename = async (n: NoteNode) => {
    const current = n.kind === 'note' ? n.title : n.name;
    const name = await promptDialog({ title: 'Rename', label: 'Name', initial: current, validate: (v) => (v.trim() ? undefined : 'Required') });
    if (!name || name === current) return;
    try {
      const to = await notesApi.rename(project.root, n.path, name);
      await reload();
      if (selected === n.path) go('notes', to);
    } catch (e) {
      toast.error('Rename failed', errorMessage(e));
    }
  };
  const remove = async (n: NoteNode) => {
    const ok = await confirmDialog({ title: `Delete “${n.kind === 'note' ? n.title : n.name}”?`, message: 'It moves to your system trash.', confirmText: 'Move to trash', danger: true });
    if (!ok) return;
    if (n.kind === 'note' && n.format === 'notebook') await notebooksApi.shutdown(project.root, n.path).catch(() => {});
    await notesApi.remove(project.root, n.path).catch((e) => toast.error('Delete failed', errorMessage(e)));
    if (selected && (selected === n.path || selected.startsWith(`${n.path}/`))) go('notes');
    reload();
  };
  const menu = (n: NoteNode): MenuItemDef[] => [
    ...(n.kind === 'folder'
      ? [
          { label: 'New note', icon: <FilePlus />, onSelect: () => newNote(n.path) },
          { label: 'New notebook', icon: <BookOpenCheck />, onSelect: () => newNotebook(n.path) },
          { label: 'New folder', icon: <FolderPlus />, onSelect: () => newFolder(n.path) },
          { separator: true },
        ]
      : []),
    { label: 'Rename…', icon: <Pencil />, onSelect: () => rename(n) },
    ...(n.kind === 'folder' ? [] : [{ label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `notes/${n.path}`, title: n.title }) }]),
    { label: 'Reveal in file manager', icon: <FolderOpen />, onSelect: () => revealItemInDir(`${project.root}/notes/${n.path}`).catch(() => {}) },
    { separator: true },
    { label: 'Move to trash', icon: <Trash2 />, danger: true, onSelect: () => remove(n) },
  ];

  const render = (n: NoteNode, depth: number): React.ReactNode => {
    const pad = { paddingLeft: 8 + depth * 14 };
    if (n.kind === 'folder') {
      const open = !closed[n.path];
      return (
        <div key={n.path}>
          <div className="tree-row folder" style={pad} onClick={() => setClosed({ ...closed, [n.path]: open })} onContextMenu={(e) => ctx.open(e, menu(n))}>
            <ChevronRight className={clsx('chev', open && 'open')} />
            {open ? <FolderOpen className="folder-icon" /> : <Folder className="folder-icon" />}
            <span className="truncate">{n.name}</span>
          </div>
          {open && n.children.map((c) => render(c, depth + 1))}
        </div>
      );
    }
    return (
      <div
        key={n.path}
        className={clsx('tree-row', selected === n.path && 'on')}
        style={pad}
        onClick={(e) => go('notes', n.path, e.ctrlKey || e.metaKey ? { newTab: true } : undefined)}
        onDoubleClick={() => go('notes', n.path, { preview: false })}
        onAuxClick={(e) => e.button === 1 && go('notes', n.path, { newTab: true })}
        onContextMenu={(e) => ctx.open(e, menu(n))}
        title={n.path}
      >
        {n.format === 'notebook' ? <BookOpenCheck size={13} className="subtle shrink-0" /> : <NotebookText size={13} className="subtle shrink-0" />}
        <span className="truncate grow">{n.title}</span>
        {n.tags.slice(0, 2).map((t) => (
          <span key={t} className="tag" style={{ height: 16, fontSize: 'var(--fs-2xs)' }}>
            {t}
          </span>
        ))}
      </div>
    );
  };

  const requests = useMemo(() => flattenRequests(requestTree), [requestTree]);

  return (
    <div className="split">
      <SidePane
        title="Notes"
        actions={
          <>
            <AiAssistButton feature="summarizeNotes" compact title="Summarize all notes (local AI)" label="Summarize notes" input={() => ({ paths: [] })} />
            <Menu
              align="right"
              items={[
                { label: 'New note', icon: <FilePlus />, onSelect: () => newNote('') },
                { label: 'New notebook', icon: <BookOpenCheck />, onSelect: () => newNotebook('') },
                { label: 'New folder', icon: <FolderPlus />, onSelect: () => newFolder('') },
              ]}
              trigger={(p) => (
                <button className="icon-btn" {...p} title="New…" aria-label="New note">
                  <FilePlus />
                </button>
              )}
            />
          </>
        }
        toolbar={<SearchInput value={q} onChange={setQ} placeholder="Search notes" className="grow" />}
      >
        <div className="tree" onContextMenu={(e) => ctx.open(e, [{ label: 'New note', icon: <FilePlus />, onSelect: () => newNote('') }, { label: 'New notebook', icon: <BookOpenCheck />, onSelect: () => newNotebook('') }, { label: 'New folder', icon: <FolderPlus />, onSelect: () => newFolder('') }])}>
          {q.trim() ? (
            hits.length ? (
              hits.map((h) => (
                <div key={h.path} className={clsx('list-item', selected === h.path && 'on')} style={{ flexDirection: 'column', alignItems: 'flex-start', gap: 2 }} onClick={() => go('notes', h.path)}>
                  <span className="strong small">{h.title}</span>
                  <span className="subtle xs" style={{ whiteSpace: 'normal' }}>
                    {h.snippet}
                  </span>
                </div>
              ))
            ) : (
              <div className="empty small">No notes match “{q}”.</div>
            )
          ) : tree?.kind === 'folder' && tree.children.length ? (
            tree.children.map((c) => render(c, 0))
          ) : (
            <div className="empty small">
              <p>Notes are Markdown files and runnable notebooks in this project’s notes/ folder — committed and shared with your team.</p>
              <button className="btn btn-sm btn-primary" onClick={() => newNote('')}>
                <FilePlus /> New note
              </button>
            </div>
          )}
        </div>
        {ctx.node}
      </SidePane>
      <div className="pane fill">
        {selected?.endsWith('.ipynb') ? (
          <NotebookEditor key={`${project.root}|${selected}`} root={project.root} path={selected} onSaved={reload} />
        ) : selected ? (
          <NoteEditor key={`${project.root}|${selected}`} root={project.root} path={selected} requests={requests} onSaved={reload} />
        ) : (
          <Empty icon={<NotebookText />} title="Shared project notes">
            Write runbooks, decisions and API quirks next to the requests they describe. Notes are plain Markdown in <code className="code-inline">notes/</code>, so they’re reviewed in pull requests and the local AI can
            answer questions from them.
          </Empty>
        )}
      </div>
    </div>
  );
}

function NoteEditor({ root, path, requests, onSaved }: { root: string; path: string; requests: { path: string; name: string; method: string }[]; onSaved(): void }) {
  const [text, setText] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>(() => (localStorage.getItem('devall.noteMode') as Mode) ?? 'split');
  const [saved, setSaved] = useState(true);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const latest = useRef('');
  const openLink = useProjectLinkOpener();
  const keepView = useKeepView();

  useEffect(() => {
    notesApi
      .read(root, path)
      .then((t) => {
        latest.current = t;
        setText(t);
      })
      .catch((e) => toast.error('Could not open note', errorMessage(e)));
  }, [root, path]);

  const flush = useCallback(() => {
    clearTimeout(timer.current);
    return notesApi
      .save(root, path, latest.current)
      .then(() => {
        setSaved(true);
        onSaved();
      })
      .catch((e) => toast.error('Save failed', errorMessage(e)));
  }, [root, path, onSaved]);

  useEffect(() => () => void (!saved && flush()), [saved, flush]);

  // Edited on disk (an agent over MCP, another editor, git): reload, unless typing here is pending —
  // then autosave would overwrite the other change, so ask.
  const fsVersion = useFsVersion((s) => s.v);
  const savedRef = useRef(saved);
  savedRef.current = saved;
  useEffect(() => {
    if (!fsVersion || !useFsVersion.getState().paths.includes(`notes/${path}`)) return;
    void notesApi.read(root, path).then((disk) => {
      if (disk === latest.current) return;
      const reload = () => {
        clearTimeout(timer.current);
        latest.current = disk;
        setText(disk);
        setSaved(true);
      };
      if (savedRef.current) reload();
      else {
        clearTimeout(timer.current);
        toast.info('This note changed on disk', 'Your unsaved edits are kept until you choose.', { label: 'Load the disk version', run: reload });
      }
    });
  }, [fsVersion, root, path]);

  const change = (v: string) => {
    latest.current = v;
    setText(v);
    setSaved(false);
    keepView();
    clearTimeout(timer.current);
    timer.current = setTimeout(flush, SAVE_MS);
  };

  if (text === null) return null;
  const setModeP = (m: Mode) => {
    setMode(m);
    localStorage.setItem('devall.noteMode', m);
  };
  return (
    <div className="page">
      <div className="page-header">
        <span className="mono subtle xs truncate">notes/{path}</span>
        <span className="subtle xs">{saved ? 'Saved' : 'Saving…'}</span>
        <span className="spacer" />
        <AiAssistButton feature="summarizeNotes" label="Summarize" title="Summarize this note (local AI)" align="right" input={() => ({ paths: [path] })} />
        <Menu
          align="right"
          items={() =>
            requests.length
              ? requests.slice(0, 60).map((r) => ({ label: `${r.method} ${r.name}`, onSelect: () => change(`${latest.current.replace(/\s*$/, '')}\n\n[${r.method} ${r.name}](http/${r.path})\n`) }))
              : [{ label: 'No requests in this project', disabled: true }]
          }
          trigger={(p) => (
            <button className="btn btn-sm btn-ghost" {...p} title="Append a link to a saved request">
              <Link2 /> Link request
            </button>
          )}
        />
        <Segmented
          value={mode}
          onChange={setModeP}
          options={[
            { value: 'edit', label: 'Edit' },
            { value: 'split', label: 'Split' },
            { value: 'preview', label: 'Preview' },
          ]}
        />
      </div>
      <div className={clsx('note-body', mode)}>
        {mode !== 'preview' && (
          <div className="note-editor">
            <CodeEditor value={text} onChange={change} language="markdown" onSave={flush} lineNumbers={false} />
          </div>
        )}
        {mode !== 'edit' && (
          <div className="note-preview">
            <NotePreview root={root} path={path} text={text} onProjectLink={(href) => openLink(href, path)} />
          </div>
        )}
      </div>
    </div>
  );
}

/** Splits YAML front matter off a note (tags are shown as chips, not rendered as Markdown). */
export function splitFrontMatter(text: string): { tags: string[]; body: string } {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(text);
  if (!m) return { tags: [], body: text };
  const line = m[1].split(/\r?\n/).find((l) => l.trim().startsWith('tags:')) ?? '';
  const raw = line.replace(/^\s*tags:\s*/, '').trim();
  const tags = raw
    .replace(/^\[|\]$/g, '')
    .split(',')
    .map((t) => t.trim().replace(/^['"]|['"]$/g, ''))
    .filter(Boolean);
  return { tags, body: text.slice(m[0].length) };
}

function NotePreview({ root, path, text, onProjectLink }: { root: string; path: string; text: string; onProjectLink(path: string): void }) {
  const { tags, body } = useMemo(() => splitFrontMatter(text), [text]);
  const renderCodeBlock = useRunnableCodeBlocks(root, path);
  return (
    <>
      {tags.length > 0 && (
        <div className="row-sm wrap mb-4">
          {tags.map((t) => (
            <span key={t} className="tag">
              {t}
            </span>
          ))}
        </div>
      )}
      <Markdown source={body} onProjectLink={onProjectLink} renderCodeBlock={renderCodeBlock} />
    </>
  );
}
