import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import {
  AppWindow,
  ArrowRightLeft,
  AtSign,
  CalendarDays,
  ChevronDown,
  ChevronUp,
  Copy,
  ExternalLink,
  File as FileIcon,
  FileArchive,
  FileCode,
  FileImage,
  FileMusic,
  FilePlay,
  FileSpreadsheet,
  FileText,
  FolderKanban,
  FolderOpen,
  Globe,
  GitBranch,
  Hash,
  History,
  Inbox,
  Link2,
  ListTree,
  MessageSquare,
  MoreHorizontal,
  PanelRight,
  Paperclip,
  Plus,
  Repeat,
  Target,
  Trash2,
  X,
} from 'lucide-react';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
import { Markdown } from '@/components/Markdown';
import { BacklinksSection } from '@/components/links/Backlinks';
import { copyLinkItems, LinkChip, useFollowLink } from '@/components/links/LinkChip';
import { pickLink } from '@/components/links/LinkPicker';
import { useTextareaLinks } from '@/components/links/useTextareaLinks';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Modal } from '@/components/ui/Modal';
import { copyText, formatBytes, isMac, RelTime, Resizer } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { reposApi } from '@/lib/api/git';
import { todosApi, type Attachment, type Snapshot, type TaskList, type TaskView } from '@/lib/api/todos';
import { errorMessage } from '@/lib/ipc';
import { linkShortcutLabel } from '@/lib/links/insert';
import { describe } from '@/lib/links/registry';
import { formatLink, isDevallLink } from '@/lib/links/syntax';
import { resolveProjectLink } from '@/features/notes/links';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';
import { useNav } from '@/store/nav';
import { TODO_DETAIL, usePrefs } from '@/store/prefs';
import { attachFiles, deleteTasks, moveTasks, pickAttachments, removeAttachment, setStatus, updateTasks, useTodos } from '@/store/todos';
import { useWorkspace } from '@/store/workspace';
import { statusOf } from './boardModel';
import { DuePicker } from './DuePicker';
import { QuickAddInput } from './QuickAddInput';
import { CheckCircle, Composer, InlineText } from './TaskRow';
import { LabelPicker, MovePicker, PriorityIcon, priorityItems, StatusDot, statusItems } from './pickers';
import { usePopover } from './Popover';
import { appDocument, classifyLink, colorVar, dayLabel, dueInfo, linkLabel, moveCursor, parseYmd, ruleText, timeLabel, todayYmd, treeRows } from './lib';
import { useTodoUi } from './ui';

const REVEAL_LABEL = isMac ? 'Reveal in Finder' : typeof navigator !== 'undefined' && navigator.platform.startsWith('Win') ? 'Show in Explorer' : 'Show in folder';

/**
 * Following a task's links: `devall:` links open the item in /dev/all (to the side with
 * Ctrl/Cmd or the middle button); URLs open in the browser; files are revealed in Explorer/Finder —
 * absolute paths as they are, project-relative ones (`http/users/list.req.yaml`, `notes/x.md`)
 * inside the task's project. Project documents can also open in /dev/all when that project is the
 * open one.
 */
function useLinkActions(task: TaskView, list: TaskList | undefined, snap: Snapshot) {
  const project = useWorkspace((s) => s.project);
  const go = useNav((s) => s.go);
  const openProjectLink = useProjectLinkOpener();
  const follow = useFollowLink();
  const projectList = list?.kind === 'project';
  const inOpenProject = !!project && (!projectList || list?.id === snap.projectList);
  // A project's list resolves against that project's folder (even when it isn't open); the Inbox
  // and the user's lists against the open project.
  const root = projectList ? (list?.projectRoot ?? (inOpenProject ? project?.root : undefined)) : project?.root;
  const target = (l: string) => classifyLink(l, root);
  const inApp = (l: string) => {
    const t = target(l);
    return t.kind !== 'url' && t.kind !== 'repo' && !!t.rel && inOpenProject && resolveProjectLink(t.rel).kind !== 'unknown';
  };
  const openInApp = (l: string) => {
    const t = target(l);
    if (t.kind === 'url' || t.kind === 'repo' || !t.rel) return;
    const doc = appDocument(t.rel);
    if (doc) go(doc.section, doc.param);
    else openProjectLink(t.rel);
  };
  const reveal = (l: string) => {
    const t = target(l);
    if (t.kind === 'file') void revealItemInDir(t.path).catch((e) => toast.error(`Could not show ${t.path}`, errorMessage(e)));
    // `repo:<name>/<path>`: the app knows where that repo is on this machine.
    else if (t.kind === 'repo') {
      if (!root) toast.info('Open the project to follow this link', `${l} is in one of a project's repositories, and this task's project isn't known.`);
      else
        void reposApi
          .resolve(root, l)
          .then((abs) => revealItemInDir(abs))
          .catch((e) => toast.error(`Could not show ${l}`, errorMessage(e)));
    } else if (t.kind === 'unresolved') toast.info('Open the project to follow this link', `${l} is relative to a project folder, and this task's project isn't known.`);
  };
  const browse = (url: string) => void openUrl(url).catch((e) => toast.error('Could not open the link', errorMessage(e)));
  return {
    target,
    inApp,
    openInApp,
    reveal,
    /** A click on a link: an app item opens; else browser / file manager, Ctrl/Cmd+click opens project documents in /dev/all. */
    activate(l: string, e?: { ctrlKey: boolean; metaKey: boolean; button?: number }) {
      if (isDevallLink(l)) return follow(l, e);
      const t = target(l);
      if (t.kind === 'url') browse(t.url);
      else if ((e?.ctrlKey || e?.metaKey) && inApp(l)) openInApp(l);
      else reveal(l);
    },
    /** Links inside Markdown (description, comments): project documents open in /dev/all, other files are revealed. */
    fromMarkdown(l: string) {
      const t = target(l);
      if (t.kind === 'url') browse(t.url);
      else if (inApp(l)) openInApp(l);
      else reveal(l);
    },
    menu(l: string): MenuItemDef[] {
      const remove: MenuItemDef = { label: 'Remove link', icon: <X />, danger: true, onSelect: () => void updateTasks([task.id], { links: (task.links ?? []).filter((x) => x !== l) }) };
      if (isDevallLink(l))
        return [
          { label: 'Open', icon: <AppWindow />, onSelect: () => follow(l) },
          { label: 'Open to the side', icon: <PanelRight />, hint: isMac ? '⌘-click' : 'Ctrl+click', onSelect: () => follow(l, { ctrlKey: true, metaKey: false }) },
          ...copyLinkItems(l, describe(l).title),
          { separator: true },
          remove,
        ];
      const t = target(l);
      return [
        ...(t.kind === 'url'
          ? [{ label: 'Open in browser', icon: <Globe />, onSelect: () => browse(t.url) }]
          : [
              { label: REVEAL_LABEL, icon: <FolderOpen />, onSelect: () => reveal(l) },
              { label: 'Open in /dev/all', icon: <AppWindow />, hint: isMac ? '⌘-click' : 'Ctrl+click', disabled: !inApp(l), onSelect: () => openInApp(l) },
            ]),
        { label: 'Copy link', icon: <Copy />, onSelect: () => void copyText(t.kind === 'file' ? t.path : l) },
        { separator: true },
        remove,
      ];
    },
  };
}

type LinkActions = ReturnType<typeof useLinkActions>;

function Title({ task }: { task: TaskView }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(task.content);
  const [ignore, setIgnore] = useState<string[]>([]);
  const done = useRef(false);
  useEffect(() => {
    setText(task.content);
    setEditing(false);
  }, [task.id, task.content]);
  const save = async () => {
    if (done.current) return;
    done.current = true;
    if (text.trim() && text.trim() !== task.content) await useTodos.getState().apply({ type: 'edit', id: task.id, text, ignore });
    setEditing(false);
  };
  return (
    <div className="td-detail-title">
      <CheckCircle task={task} />
      {editing ? (
        <div className="grow" style={{ paddingTop: 2 }}>
          <QuickAddInput value={text} onChange={setText} ignore={ignore} onIgnore={setIgnore} listId={task.listId} autoFocus big onSubmit={save} onBlur={save} onCancel={() => ((done.current = true), setEditing(false), setText(task.content))} />
        </div>
      ) : (
        <div
          className="td-content-big"
          onClick={() => {
            done.current = false;
            setEditing(true);
          }}
          title="Click to edit (quick-add syntax works)"
        >
          <InlineText text={task.content} />
        </div>
      )}
    </div>
  );
}

/** "Link to…" button next to a Markdown field (the picker inserts `[Title](devall:…)` at the cursor). */
function LinkButton({ onClick }: { onClick(): void }) {
  return (
    <button className="icon-btn sm td-field-link" onMouseDown={(e) => e.preventDefault()} onClick={onClick} title={`Link to a request, note, to-do… (${linkShortcutLabel(isMac)}, or type [[)`} aria-label="Insert link">
      <Link2 />
    </button>
  );
}

function Description({ task, links }: { task: TaskView; links: LinkActions }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(task.description ?? '');
  const linker = useTextareaLinks(setText);
  useEffect(() => {
    setText(task.description ?? '');
    setEditing(false);
  }, [task.id, task.description]);
  const save = () => {
    setEditing(false);
    if (text.trimEnd() !== (task.description ?? '')) void updateTasks([task.id], { description: text });
  };
  if (editing)
    return (
      <div className="td-field">
        <textarea
          ref={linker.ref}
          className="textarea"
          autoFocus
          rows={Math.min(16, Math.max(4, text.split('\n').length + 1))}
          value={text}
          placeholder="Description — Markdown: **bold**, `code`, - lists; [[ links to anything"
          onChange={linker.onChange}
          // The link picker takes focus for a moment: that's not leaving the field.
          onBlur={() => !linker.picking.current && save()}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (linker.onKeyDown(e)) return;
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
            if (e.key === 'Escape') {
              setText(task.description ?? '');
              setEditing(false);
            }
          }}
        />
        <LinkButton onClick={() => void linker.insert()} />
      </div>
    );
  return (
    <div className={clsx('td-desc-view', !task.description?.trim() && 'placeholder')} onClick={(e) => !(e.target as HTMLElement).closest('a') && setEditing(true)} title="Click to edit">
      {task.description?.trim() ? <Markdown source={task.description} onProjectLink={links.fromMarkdown} /> : 'Add a description (Markdown)…'}
    </div>
  );
}

function Links({ task, links }: { task: TaskView; links: LinkActions }) {
  /** "Add link…": anything in the app, or a typed URL / file path. */
  const add = async (e: React.MouseEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const picked = await pickLink({ anchor: { x: r.left, y: r.bottom }, freeform: true, placeholder: 'Link to a request, note, to-do, page… or paste a URL or file path' });
    const v = picked?.link.trim();
    if (v) void updateTasks([task.id], { addLinks: [v] });
  };
  return (
    <div className="td-links">
      {(task.links ?? []).map((l) => {
        if (isDevallLink(l))
          return (
            <div key={l} className="td-link">
              <button className="td-link-label td-link-chip" onClick={(e) => links.activate(l, e)} onAuxClick={(e) => e.button === 1 && links.activate(l, e)} onMouseDown={(e) => e.button === 1 && e.preventDefault()}>
                <LinkChip href={l} />
              </button>
              <Menu
                align="right"
                items={() => links.menu(l)}
                trigger={(p) => (
                  <button className="icon-btn sm" {...p} aria-label="Link actions" title="Link actions">
                    <MoreHorizontal />
                  </button>
                )}
              />
            </div>
          );
        const t = links.target(l);
        const hint =
          t.kind === 'url'
            ? 'Open in browser'
            : t.kind === 'file'
              ? `${REVEAL_LABEL}${links.inApp(l) ? ` · ${isMac ? '⌘' : 'Ctrl'}+click: open in /dev/all` : ''}`
              : t.kind === 'repo'
                ? `${REVEAL_LABEL} (in the project's ${t.repo} repository)`
                : 'Relative to a project folder';
        return (
          <div key={l} className="td-link">
            {t.kind === 'url' ? <ExternalLink /> : t.kind === 'file' ? <FileText /> : t.kind === 'repo' ? <GitBranch /> : <Hash />}
            <button className="td-link-label" onClick={(e) => links.activate(l, e)} title={`${t.kind === 'file' ? t.path : l}\n${hint}`}>
              {linkLabel(l)}
            </button>
            <Menu
              align="right"
              items={() => links.menu(l)}
              trigger={(p) => (
                <button className="icon-btn sm" {...p} aria-label="Link actions" title="Link actions">
                  <MoreHorizontal />
                </button>
              )}
            />
          </div>
        );
      })}
      <button className="td-prop-btn placeholder" onClick={(e) => void add(e)} title="Anything in /dev/all (requests, notes, to-dos, environments, stubs, pages…), a URL or a file">
        <Plus /> <span className="truncate">Add link…</span>
      </button>
    </div>
  );
}

// ── attachments ────────────────────────────────────────────────────────────────────────────────

/** Image types the webview renders (others show as files). */
const PREVIEWABLE = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/bmp', 'image/svg+xml', 'image/x-icon', 'image/avif'];
const PREVIEW_MAX = 15 * 1024 * 1024;
const previewable = (a: Attachment) => PREVIEWABLE.includes(a.mime) && a.size <= PREVIEW_MAX;

/** Blob URLs of loaded images, newest last (a small cache shared by thumbnails and the preview). */
const urls = new Map<string, Promise<string>>();

function attachmentUrl(taskId: string, id: string, mime: string): Promise<string> {
  const key = `${taskId}/${id}`;
  let p = urls.get(key);
  if (!p) {
    p = todosApi.attachmentBytes(taskId, id).then((buf) => URL.createObjectURL(new Blob([buf], { type: mime })));
    p.catch(() => urls.delete(key));
    urls.set(key, p);
    if (urls.size > 60) {
      const [oldKey, old] = urls.entries().next().value!;
      urls.delete(oldKey);
      void old.then((u) => URL.revokeObjectURL(u)).catch(() => {});
    }
  }
  return p;
}

function useAttachmentUrl(taskId: string, a: Attachment): { url: string | null; failed: boolean } {
  const [state, setState] = useState<{ key: string; url: string | null; failed: boolean }>({ key: '', url: null, failed: false });
  const { id, mime } = a;
  const key = `${taskId}/${id}`;
  useEffect(() => {
    let live = true;
    attachmentUrl(taskId, id, mime)
      .then((url) => live && setState({ key, url, failed: false }))
      .catch(() => live && setState({ key, url: null, failed: true }));
    return () => {
      live = false;
    };
  }, [taskId, id, mime, key]);
  return state.key === key ? state : { url: null, failed: false };
}

function fileIcon(a: Attachment) {
  const m = a.mime;
  if (m.startsWith('image/')) return <FileImage />;
  if (m.startsWith('video/')) return <FilePlay />;
  if (m.startsWith('audio/')) return <FileMusic />;
  if (/zip|gzip|tar|7z/.test(m)) return <FileArchive />;
  if (/spreadsheet|excel|csv/.test(m)) return <FileSpreadsheet />;
  if (/json|yaml|xml|html/.test(m)) return <FileCode />;
  if (m.startsWith('text/') || m === 'application/pdf' || /word|presentation|powerpoint/.test(m)) return <FileText />;
  return <FileIcon />;
}

function openAttachment(taskId: string, a: Attachment) {
  todosApi
    .openAttachment(taskId, a.id)
    .then((opened) => !opened && toast.info(`${a.name} is a program`, 'It was shown in its folder instead of being run.'))
    .catch((e) => toast.error(`Could not open ${a.name}`, errorMessage(e)));
}

function revealAttachment(taskId: string, a: Attachment) {
  todosApi.revealAttachment(taskId, a.id).catch((e) => toast.error(`Could not show ${a.name}`, errorMessage(e)));
}

function attachmentMenu(taskId: string, a: Attachment): MenuItemDef[] {
  return [
    { label: 'Open', icon: <ExternalLink />, onSelect: () => openAttachment(taskId, a) },
    { label: REVEAL_LABEL, icon: <FolderOpen />, onSelect: () => revealAttachment(taskId, a) },
    { separator: true },
    { label: 'Remove', icon: <Trash2 />, danger: true, onSelect: () => void removeAttachment(taskId, a) },
  ];
}

function Thumb({ taskId, a, onPreview }: { taskId: string; a: Attachment; onPreview(): void }) {
  const { url, failed } = useAttachmentUrl(taskId, a);
  return (
    <div className="td-thumb" title={`${a.name} · ${formatBytes(a.size)}`}>
      <button className="td-thumb-img" onClick={onPreview} aria-label={`Preview ${a.name}`}>
        {url ? <img src={url} alt={a.name} draggable={false} /> : failed ? <FileImage /> : null}
      </button>
      <div className="td-thumb-bar">
        <span className="truncate">{a.name}</span>
        <Menu
          align="right"
          items={() => attachmentMenu(taskId, a)}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} aria-label={`${a.name} actions`}>
              <MoreHorizontal />
            </button>
          )}
        />
      </div>
    </div>
  );
}

function Preview({ taskId, a, onClose }: { taskId: string; a: Attachment; onClose(): void }) {
  const { url, failed } = useAttachmentUrl(taskId, a);
  return (
    <Modal
      open
      onClose={onClose}
      size="xl"
      title={a.name}
      headerExtra={
        <>
          <button className="btn btn-sm btn-ghost" onClick={() => openAttachment(taskId, a)}>
            <ExternalLink /> Open
          </button>
          <button className="btn btn-sm btn-ghost" onClick={() => revealAttachment(taskId, a)}>
            <FolderOpen /> {REVEAL_LABEL}
          </button>
        </>
      }
      bodyClassName="td-preview-body"
    >
      {url ? <img className="td-preview-img" src={url} alt={a.name} /> : <span className="subtle">{failed ? 'The file is missing from this device.' : 'Loading…'}</span>}
    </Modal>
  );
}

function Attachments({ task }: { task: TaskView }) {
  const [preview, setPreview] = useState<Attachment | null>(null);
  const all = task.attachments ?? [];
  const images = all.filter(previewable);
  const files = all.filter((a) => !previewable(a));
  return (
    <div>
      <div className="td-section-title">
        <Paperclip size={13} /> Attachments <span className="count">{all.length || ''}</span>
        <span className="spacer" />
        <button className="icon-btn sm" onClick={() => void pickAttachments(task.id)} title="Attach files…" aria-label="Attach files">
          <Plus />
        </button>
      </div>
      {images.length > 0 && (
        <div className="td-thumbs">
          {images.map((a) => (
            <Thumb key={a.id} taskId={task.id} a={a} onPreview={() => setPreview(a)} />
          ))}
        </div>
      )}
      {files.map((a) => (
        <div key={a.id} className="td-file">
          {fileIcon(a)}
          <button className="td-link-label" onClick={() => openAttachment(task.id, a)} title={`Open ${a.name}`}>
            {a.name}
          </button>
          <span className="subtle xs td-file-size">{formatBytes(a.size)}</span>
          <Menu
            align="right"
            items={() => attachmentMenu(task.id, a)}
            trigger={(p) => (
              <button className="icon-btn sm" {...p} aria-label={`${a.name} actions`}>
                <MoreHorizontal />
              </button>
            )}
          />
        </div>
      ))}
      {!all.length && (
        <button className="td-add-btn" onClick={() => void pickAttachments(task.id)}>
          <Paperclip /> <span className="truncate">Attach files — or drop or paste them here</span>
        </button>
      )}
      {preview && <Preview taskId={task.id} a={preview} onClose={() => setPreview(null)} />}
    </div>
  );
}

/** Screenshots arrive as `image.png`: give them a findable name. */
function pastedName(f: File): File {
  if (f.name && !/^image\.(png|jpe?g|gif|webp|bmp)$/i.test(f.name)) return f;
  const ext = (f.type.split('/')[1] ?? 'png').replace('jpeg', 'jpg').replace(/[^a-z0-9]/gi, '') || 'png';
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return new File([f], `Pasted image ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}.${p(d.getMinutes())}.${p(d.getSeconds())}.${ext}`, { type: f.type });
}

const hasFiles = (e: React.DragEvent) => e.dataTransfer.types.includes('Files');

function Comments({ task, links }: { task: TaskView; links: LinkActions }) {
  const [text, setText] = useState('');
  const linker = useTextareaLinks(setText);
  const post = async () => {
    if (!text.trim()) return;
    const r = await useTodos.getState().apply({ type: 'addComment', id: task.id, text });
    if (r) setText('');
  };
  return (
    <div className="col-sm">
      {(task.comments ?? []).map((c) => (
        <div key={c.id} className="td-comment">
          <div className="meta">
            <RelTime ts={c.at} />
            <button className="icon-btn sm danger" onClick={() => void useTodos.getState().apply({ type: 'deleteComment', id: task.id, commentId: c.id })} aria-label="Delete comment" title="Delete comment">
              <Trash2 />
            </button>
          </div>
          <Markdown source={c.text} onProjectLink={links.fromMarkdown} />
        </div>
      ))}
      <div className="td-field">
        <textarea
          ref={linker.ref}
          className="textarea"
          rows={2}
          placeholder="Comment (Markdown, [[ links to anything) — Ctrl+Enter to post"
          value={text}
          onChange={linker.onChange}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (linker.onKeyDown(e)) return;
            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void post();
          }}
        />
        <LinkButton onClick={() => void linker.insert()} />
      </div>
      {text.trim() && (
        <button className="btn btn-sm btn-primary" style={{ alignSelf: 'flex-end' }} onClick={() => void post()}>
          Comment
        </button>
      )}
    </div>
  );
}

/**
 * The right-hand panel for the open task. Resizable (its left edge; the width is remembered);
 * `overlay` floats it over the list when the view is too narrow to show both. Files dropped on it
 * or pasted into it become attachments.
 */
export function TaskDetail({ overlay, maxWidth }: { overlay: boolean; maxWidth: number }) {
  const snap = useTodos((s) => s.snap);
  const openId = useTodos((s) => s.openId);
  const order = useTodoUi((s) => s.order);
  const composer = useTodoUi((s) => s.composer);
  const setUi = useTodoUi((s) => s.set);
  const pref = usePrefs((s) => s.todoDetailWidth);
  const setPref = usePrefs((s) => s.set);
  const [dropping, setDropping] = useState(false);
  const due = usePopover();
  const deadline = usePopover();
  const labels = usePopover();
  const move = usePopover();
  const task = snap?.tasks.find((t) => t.id === openId);
  const list = task ? snap?.lists.find((l) => l.id === task.listId) : undefined;
  const links = useLinkActions(task ?? ({} as TaskView), list, snap ?? ({} as Snapshot));
  if (!snap || !task) return null;
  const today = snap.today ?? todayYmd();
  const max = Math.max(TODO_DETAIL.min, Math.min(TODO_DETAIL.max, maxWidth));
  const width = Math.min(pref, max);
  const section = task.sectionId ? snap.sections.find((s) => s.id === task.sectionId) : undefined;
  const parent = task.parentId ? snap.tasks.find((t) => t.id === task.parentId) : undefined;
  const kids = treeRows(
    snap.tasks.filter((t) => t.parentId === task.id),
    true,
    0,
  );
  const info = dueInfo(task, today);
  const status = statusOf(task, snap.settings.columns);
  const foreign = list?.kind === 'project' && list.id !== snap.projectList ? { name: list.name, root: list.projectRoot } : undefined;
  const go = (dir: 1 | -1) => {
    const next = moveCursor(order, task.id, dir);
    if (next) {
      useTodos.getState().open(next);
      setUi({ cursor: next });
    }
  };

  return (
    <aside
      className={clsx('td-detail', overlay && 'floating', dropping && 'dropping')}
      style={{ width }}
      aria-label="Task details"
      tabIndex={-1}
      onKeyDown={(e) => e.key === 'Escape' && !(e.target as HTMLElement).closest('input, textarea') && useTodos.getState().open(null)}
      onPaste={(e) => {
        const files = [...e.clipboardData.files];
        if (!files.length) return;
        // Text pasted into a field stays text (copying from an editor can carry an image too).
        if ((e.target as HTMLElement).closest('input, textarea') && e.clipboardData.types.includes('text/plain')) return;
        e.preventDefault();
        void attachFiles(task.id, files.map(pastedName));
      }}
      onDragOver={(e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = 'copy';
        if (!dropping) setDropping(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        e.stopPropagation();
        setDropping(false);
        void attachFiles(task.id, [...e.dataTransfer.files]);
      }}
    >
      <Resizer className="td-detail-resizer" invert width={width} onChange={(w) => setPref('todoDetailWidth', w)} min={TODO_DETAIL.min} max={max} defaultValue={TODO_DETAIL.default} />
      <div className="td-detail-head">
        {list?.kind === 'inbox' ? <Inbox size={13} /> : list?.kind === 'project' ? <FolderKanban size={13} style={{ color: colorVar(list?.color) }} /> : <Hash size={13} style={{ color: colorVar(list?.color) }} />}
        <span className="truncate grow">
          {list?.name}
          {section ? ` / ${section.name}` : ''}
          {parent ? ` / ${parent.content}` : ''}
        </span>
        <Menu
          align="right"
          items={() => copyLinkItems(formatLink('todo', task.id), task.content)}
          trigger={(p) => (
            <button className="icon-btn sm" {...p} title="Copy a link to this to-do (for notes and other to-dos)" aria-label="Copy link">
              <Link2 />
            </button>
          )}
        />
        <button className="icon-btn sm" onClick={() => go(-1)} disabled={order.indexOf(task.id) <= 0} title="Previous task (K)" aria-label="Previous task">
          <ChevronUp />
        </button>
        <button className="icon-btn sm" onClick={() => go(1)} disabled={order.indexOf(task.id) < 0 || order.indexOf(task.id) >= order.length - 1} title="Next task (J)" aria-label="Next task">
          <ChevronDown />
        </button>
        <button className="icon-btn sm" onClick={() => useTodos.getState().open(null)} title="Close (Esc)" aria-label="Close details">
          <X />
        </button>
      </div>
      <div className="td-detail-body">
        {parent && (
          <button className="td-prop-btn placeholder td-parent-link" onClick={() => useTodos.getState().open(parent.id)}>
            <ListTree /> <span className="truncate">{parent.content}</span>
          </button>
        )}
        <Title task={task} />
        <Description task={task} links={links} />
        <div className="td-props">
          <span className="k">Status</span>
          <span className="v">
            <Menu
              items={statusItems(snap.settings.columns, status?.id, (c) => void setStatus([task.id], c.id, undefined, c.done && !task.done ? 'Completed' : undefined))}
              trigger={(p) => (
                <button className="td-prop-btn" {...p} title="Board column — the done column completes the task">
                  <StatusDot col={status} />
                  <span className="truncate">{status?.name ?? 'No status'}</span>
                </button>
              )}
            />
          </span>
          <span className="k">Due</span>
          <span className="v">
            <button className={clsx('td-prop-btn', !info && 'placeholder')} onClick={due.toggle}>
              {info?.recurring ? <Repeat /> : <CalendarDays />}
              <span className={clsx('truncate', info && `td-due ${info.late ? 'late' : info.tone}`)}>
                {info ? `${parseYmd(task.dueLocal ?? task.due!.date).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} (${info.text})${info.time ? ` ${info.time}` : ''}` : 'No date'}
              </span>
            </button>
            {task.due?.recurrence && <span className="subtle xs truncate">{ruleText(task.due)}</span>}
            {task.due?.timezone && <span className="subtle xs truncate">{task.due.timezone}</span>}
          </span>
          <span className="k">Deadline</span>
          <span className="v">
            <button className={clsx('td-prop-btn', !task.deadline && 'placeholder')} onClick={deadline.toggle}>
              <Target />
              <span className={clsx('truncate', task.deadline && task.deadline < today && 'td-deadline overdue')}>
                {task.deadline ? `${parseYmd(task.deadline).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: task.deadline.slice(0, 4) !== today.slice(0, 4) ? 'numeric' : undefined })} (${dayLabel(task.deadline, today)})` : 'No deadline'}
              </span>
            </button>
          </span>
          <span className="k">Priority</span>
          <span className="v">
            <Menu
              items={priorityItems(task.priority, (p) => void updateTasks([task.id], { priority: p }))}
              trigger={(p) => (
                <button className="td-prop-btn" {...p}>
                  <PriorityIcon p={task.priority} /> Priority {task.priority}
                </button>
              )}
            />
          </span>
          <span className="k">Labels</span>
          <span className="v">
            {(task.labels ?? []).map((l) => (
              <span key={l} className="tag td-tag" style={{ color: colorVar(snap.labels.find((x) => x.name === l)?.color) }}>
                <AtSign /> <span className="truncate">{l}</span>
                <button onClick={() => void updateTasks([task.id], { removeLabels: [l] })} aria-label={`Remove ${l}`}>
                  <X />
                </button>
              </span>
            ))}
            <button className="td-prop-btn placeholder" onClick={labels.toggle}>
              <Plus /> {task.labels?.length ? '' : 'Add label'}
            </button>
          </span>
          <span className="k">List</span>
          <span className="v">
            <button className="td-prop-btn" onClick={move.toggle} title="Move to another list or section">
              <ArrowRightLeft />
              <span className="truncate">
                {list?.name}
                {section ? ` / ${section.name}` : ''}
              </span>
            </button>
          </span>
          <span className="k td-k-top">Links</span>
          <span className="v">
            <Links task={task} links={links} />
          </span>
        </div>
        <BacklinksSection link={formatLink('todo', task.id)} />
        {foreign?.root && (
          <button className="btn btn-sm btn-ghost td-open-project" onClick={() => void useWorkspace.getState().openProject(foreign.root!)} title={`Open ${foreign.root} to follow links in /dev/all`}>
            <FolderOpen /> <span className="truncate">Open {foreign.name}</span>
          </button>
        )}

        <Attachments task={task} />

        <div>
          <div className="td-section-title">
            <ListTree size={13} /> Sub-tasks <span className="count">{kids.length ? `${kids.filter((k) => k.task.done).length}/${kids.length}` : ''}</span>
          </div>
          {kids.map((k) => (
            <div key={k.task.id} className={clsx('td-sub', k.task.done && 'done')}>
              <CheckCircle task={k.task} />
              <span className="grow truncate" onClick={() => useTodos.getState().open(k.task.id)}>
                <InlineText text={k.task.content} />
              </span>
              {k.task.due && <span className="subtle xs">{dayLabel(k.task.dueLocal ?? k.task.due.date, today)}</span>}
            </div>
          ))}
          {composer === `detail-sub:${task.id}` ? (
            <Composer defaults={{ parentId: task.id }} onDone={() => setUi({ composer: null })} placeholder="Sub-task" />
          ) : (
            <button className="td-add-btn" onClick={() => setUi({ composer: `detail-sub:${task.id}` })}>
              <Plus /> Add sub-task
            </button>
          )}
        </div>

        <div>
          <div className="td-section-title">
            <MessageSquare size={13} /> Comments <span className="count">{task.comments?.length || ''}</span>
          </div>
          <Comments task={task} links={links} />
        </div>

        {(task.history?.length ?? 0) > 0 && (
          <div>
            <div className="td-section-title">
              <History size={13} /> Completed {task.history!.length}×
            </div>
            <div className="col-sm xs subtle">
              {[...task.history!]
                .reverse()
                .slice(0, 8)
                .map((h) => (
                  <span key={h.at}>
                    {parseYmd(h.due).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} — done {new Date(h.at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}
                  </span>
                ))}
            </div>
          </div>
        )}

        <div className="td-detail-foot">
          <span>
            Created <RelTime ts={task.createdAt} />
          </span>
          {task.updatedAt > task.createdAt && (
            <span>
              Updated <RelTime ts={task.updatedAt} />
            </span>
          )}
          {task.completedAt && (
            <span>
              Completed <RelTime ts={task.completedAt} />
            </span>
          )}
          {task.due?.time && !task.done && <span>Reminder at {timeLabel(task.due.time)} while the app runs</span>}
          <span className="spacer" />
          <button className="btn btn-sm btn-danger-ghost" onClick={() => void deleteTasks([task.id])}>
            <Trash2 /> Delete
          </button>
        </div>
      </div>
      {dropping && (
        <div className="td-drop-overlay">
          <Paperclip /> Drop to attach to “{task.content}”
        </div>
      )}
      {due.anchor && <DuePicker anchor={due.anchor} onClose={due.close} mode="due" due={task.due} today={today} onPatch={(p) => void updateTasks([task.id], p)} />}
      {deadline.anchor && <DuePicker anchor={deadline.anchor} onClose={deadline.close} mode="deadline" deadline={task.deadline} today={today} onPatch={(p) => void updateTasks([task.id], p)} />}
      {labels.anchor && <LabelPicker anchor={labels.anchor} onClose={labels.close} selected={task.labels ?? []} onToggle={(name, on) => void updateTasks([task.id], on ? { addLabels: [name] } : { removeLabels: [name] })} />}
      {move.anchor && (
        <MovePicker
          anchor={move.anchor}
          onClose={move.close}
          current={{ listId: task.listId, sectionId: task.sectionId }}
          onPick={(listId, sectionId) => void moveTasks([task.id], { listId, sectionId, parentId: null, index: null }, `Moved to ${snap.lists.find((l) => l.id === listId)?.name ?? 'list'}`)}
        />
      )}
    </aside>
  );
}
