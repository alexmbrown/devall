import { useCallback, useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { BookOpenCheck, ChevronRight, FilePlus, FolderOpen, FolderPlus, Folder, History, Link2, NotebookText, Pencil, Trash2 } from 'lucide-react';
import { openLocalHistory } from '@/features/history/LocalHistoryDialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { CodeEditor } from '@/components/editor/CodeEditor';
import { monaco } from '@/components/editor/monaco';
import { MarkdownEditor, type MarkdownEditorHandle } from '@/components/markdown/MarkdownEditor';
import { BacklinksSection } from '@/components/links/Backlinks';
import { copyLinkItems } from '@/components/links/LinkChip';
import { pickLink } from '@/components/links/LinkPicker';
import { Menu, useContextMenu, type MenuItemDef } from '@/components/ui/Menu';
import { Empty, isMac, SearchInput, Segmented } from '@/components/ui/misc';
import { SidePane } from '@/components/layout/SidePane';
import { confirmDialog, promptDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { notesApi, type NoteHit, type NoteNode } from '@/lib/api/notes';
import { notebooksApi } from '@/lib/api/notebooks';
import { reposApi } from '@/lib/api/git';
import { NotebookEditor, createNotebook, useRunnableCodeExtras } from '@/features/notebooks';
import { AiAssistButton } from '@/features/ai';
import { openStubFile } from '@/features/mocks';
import { noteImageUrl } from './images';
import { resolveProjectLink } from './links';
import { linkShortcutLabel, wikiTrigger } from '@/lib/links/insert';
import { formatLink, markdownLink } from '@/lib/links/syntax';
import { useKeepView, useNav, useViewState } from '@/store/nav';
import { useFsVersion, useWorkspace } from '@/store/workspace';

/** Notes are edited in place (`rich`, the default) or as Markdown source. */
type Mode = 'rich' | 'source';
const SAVE_MS = 700;

/**
 * Opens project links from rendered Markdown: requests (`http/…`), notes (also relative to the
 * current note: `other.md`, `../ops/runbook.md`), mock stubs (`mocks/mappings/…`), webhook inboxes,
 * load tests, connections, saved queries, fake-data schemas and environments. `repo:<name>/<path>`
 * links (files in one of the project's repositories) are revealed in Explorer/Finder.
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
        case 'repo': {
          const root = useWorkspace.getState().project?.root;
          if (root)
            void reposApi
              .resolve(root, href)
              .then((abs) => revealItemInDir(abs))
              .catch((e) => toast.error(`Could not show ${href}`, errorMessage(e)));
          break;
        }
        default:
          toast.info('Not a link to something in this project', href);
      }
    },
    [go],
  );
}

export default function NotesPage() {
  const project = useWorkspace((s) => s.project!);
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
    ...(n.kind === 'folder' ? [] : [{ label: 'Local history…', icon: <History />, onSelect: () => openLocalHistory({ root: project.root, path: `notes/${n.path}`, title: n.title }) }, ...copyLinkItems(formatLink('note', n.path), n.title)]),
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
          <NoteEditor key={`${project.root}|${selected}`} root={project.root} path={selected} onSaved={reload} />
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

/**
 * A Markdown note: edited in place (WYSIWYG, the default) or as source (Monaco). Autosaves; only
 * edits are written (opening a note never rewrites it) and untouched parts keep their exact bytes.
 */
function NoteEditor({ root, path, onSaved }: { root: string; path: string; onSaved(): void }) {
  const [text, setText] = useState<string | null>(null);
  const [mode, setMode] = useViewState<Mode>(`note-mode|${path}`, 'rich');
  const [saved, setSaved] = useState(true);
  const rich = useRef<MarkdownEditorHandle>(null);
  const runnable = useRunnableCodeExtras(root, path);
  const resolveImage = useCallback((src: string) => noteImageUrl(root, path, src), [root, path]);
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

  // Links to anything: `[[`, the Link button or Ctrl+Alt+K / ⌘⌥K open the picker; the pick goes in
  // at the cursor as `[Title](devall:…)` (a selection becomes the link text).
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const picking = useRef(false);
  const insertLink = async (range?: monaco.IRange) => {
    if (picking.current) return;
    if (mode === 'rich' && rich.current) {
      rich.current.insertLink();
      return;
    }
    const ed = editorRef.current;
    const model = ed?.getModel();
    if (!ed || !model) {
      // No editor yet: append it.
      const picked = await pickLink();
      if (picked) change(`${latest.current.replace(/\s*$/, '')}\n\n${markdownLink(picked.title, picked.link)}\n`);
      return;
    }
    const sel = range ?? ed.getSelection();
    if (!sel) return;
    const selected = range ? '' : model.getValueInRange(sel).trim();
    const pos = ed.getPosition();
    const vis = pos ? ed.getScrolledVisiblePosition(pos) : null;
    const box = ed.getDomNode()?.getBoundingClientRect();
    picking.current = true;
    const picked = await pickLink({ anchor: vis && box ? { x: box.left + vis.left, y: box.top + vis.top + vis.height } : undefined });
    picking.current = false;
    ed.focus();
    if (!picked) return;
    ed.executeEdits('devall-link', [{ range: sel, text: markdownLink(selected && !selected.includes('\n') ? selected : picked.title, picked.link), forceMoveMarkers: true }]);
    ed.pushUndoStop();
  };
  const insertRef = useRef(insertLink);
  insertRef.current = insertLink;
  const onEditor = (ed: monaco.editor.IStandaloneCodeEditor) => {
    editorRef.current = ed;
    ed.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyMod.Alt | monaco.KeyCode.KeyK, () => void insertRef.current());
    ed.onDidChangeModelContent((e) => {
      const c = e.changes[0];
      if (e.isUndoing || e.isRedoing || e.isFlush || e.changes.length !== 1 || !c.text.includes('[') || c.text.length > 2) return;
      // After the cursor moved past what was typed (and an auto-closed `]`).
      setTimeout(() => {
        const model = ed.getModel();
        const at = ed.getPosition();
        if (!model || !at) return;
        const t = wikiTrigger(model.getValue(), model.getOffsetAt(at));
        if (!t) return;
        const a = model.getPositionAt(t.from);
        const b = model.getPositionAt(t.to);
        void insertRef.current(new monaco.Range(a.lineNumber, a.column, b.lineNumber, b.column));
      }, 0);
    });
  };
  useEffect(() => () => void (editorRef.current = null), []);

  if (text === null) return null;
  const switchMode = (m: Mode) => {
    rich.current?.flush();
    setMode(m);
  };
  return (
    <div className="page">
      <div className="page-header">
        <span className="mono subtle xs truncate">notes/{path}</span>
        <span className="subtle xs">{saved ? 'Saved' : 'Saving…'}</span>
        <span className="spacer" />
        <AiAssistButton feature="summarizeNotes" label="Summarize" title="Summarize this note (local AI)" align="right" input={() => ({ paths: [path] })} />
        <button className="btn btn-sm btn-ghost" onMouseDown={(e) => e.preventDefault()} onClick={() => void insertLink()} title={`Link to a request, note, to-do, environment… (${linkShortcutLabel(isMac)}, or type [[)`}>
          <Link2 /> Link to…
        </button>
        <Segmented
          value={mode}
          onChange={switchMode}
          options={[
            { value: 'rich', label: 'Edit', title: 'Edit the note as it reads (type / for blocks, Markdown shortcuts work)' },
            { value: 'source', label: 'Source', title: 'Edit the Markdown source' },
          ]}
        />
      </div>
      <div className={clsx('note-body', mode)}>
        {mode === 'rich' ? (
          <MarkdownEditor
            variant="document"
            handleRef={rich}
            value={text}
            onChange={change}
            onSave={() => void flush()}
            onProjectLink={(href) => openLink(href, path)}
            resolveImage={resolveImage}
            codeBlockExtras={runnable}
            placeholder="Write here — type / for blocks, [[ to link a request, note or to-do…"
            ariaLabel={`Note ${path}`}
          />
        ) : (
          <div className="note-editor">
            <CodeEditor value={text} onChange={change} language="markdown" onSave={flush} lineNumbers={false} onMount={onEditor} />
          </div>
        )}
      </div>
      <BacklinksSection link={formatLink('note', path)} className="note-backlinks" />
    </div>
  );
}
