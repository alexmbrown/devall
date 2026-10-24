import { create } from 'zustand';
import { listen } from '@tauri-apps/api/event';
import { MAX_ATTACHMENT_BYTES, todosApi, type Attachment, type NewTask, type Snapshot, type Task, type TaskPatch, type TaskView, type TodoApplyResult, type TodoAttachResult, type TodoOp } from '@/lib/api/todos';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { attention, dayLabel, parseViewKey, subtree, viewKey, type MoveTarget, type ViewRef } from '@/features/todos/lib';
import { useNav } from './nav';
import { useWorkspace } from './workspace';

/** What an undo puts back: tasks to restore as they were, ids of tasks to remove (undo an add). */
export interface Undo {
  restore?: Task[];
  remove?: string[];
}

/**
 * Personal to-dos (`todos/todos.json` in the app data folder): the snapshot for the open project
 * (`root`; its list is in scope), the selected view and the open task. Every change goes through
 * `apply`, which returns the next snapshot.
 */
interface TodoState {
  root: string | null;
  snap: Snapshot | null;
  /** Open tasks due today or overdue, all lists (what the sidebar badge counts). */
  items: TaskView[];
  /** `items.length`. */
  badge: number;
  view: ViewRef | null;
  /** Task shown in the detail panel. */
  openId: string | null;
  load(root: string | null): Promise<void>;
  refresh(): Promise<void>;
  /** Applies an op; `message` + `undo` show a toast with an Undo button. */
  apply(op: TodoOp, message?: string, undo?: Undo): Promise<TodoApplyResult | null>;
  /** Takes a snapshot returned by another command (attachments). */
  receive(snap: Snapshot): void;
  setView(v: ViewRef): void;
  open(id: string | null): void;
}

const VIEW_KEY = 'devall.todos.view';

function storedView(): ViewRef | null {
  try {
    const v = localStorage.getItem(VIEW_KEY);
    return v ? parseViewKey(v) : null;
  } catch {
    return null;
  }
}

/** Drops a view whose list/label/filter no longer exists. */
function validView(v: ViewRef | null, s: Snapshot): ViewRef | null {
  if (!v) return null;
  if (v.kind === 'list') return s.lists.some((l) => l.id === v.id) ? v : null;
  if (v.kind === 'label') return s.labels.some((l) => l.name === v.name) ? v : null;
  if (v.kind === 'filter') return s.filters.some((f) => f.id === v.id) ? v : null;
  return v;
}

let seq = 0;

export const useTodos = create<TodoState>((set, get) => {
  const setSnap = (snap: Snapshot, projectChanged = false) => {
    const items = attention(snap.tasks, snap.today);
    let view = validView(get().view, snap);
    // Another project was opened: its list replaces the previous project's.
    const cur = view;
    if (projectChanged && cur?.kind === 'list' && snap.lists.find((l) => l.id === cur.id)?.kind === 'project' && cur.id !== snap.projectList) view = null;
    view ??= snap.projectList ? { kind: 'list' as const, id: snap.projectList } : { kind: 'today' as const };
    const openId = get().openId && snap.tasks.some((t) => t.id === get().openId) ? get().openId : null;
    set({ snap, items, badge: items.length, view, openId });
  };
  return {
    root: null,
    snap: null,
    items: [],
    badge: 0,
    view: storedView(),
    openId: null,
    async load(root) {
      const my = ++seq;
      const changed = root !== get().root;
      if (changed) set({ root });
      try {
        const snap = await todosApi.snapshot(root);
        if (my === seq) setSnap(snap, changed);
      } catch (e) {
        if (my === seq) toast.error('Could not load to-dos', errorMessage(e));
      }
    },
    refresh: () => get().load(get().root),
    async apply(op, message, undo) {
      seq++;
      try {
        const r = await todosApi.apply(get().root, op);
        // The write is the newest state: it wins over any load still in flight (seq).
        setSnap(r.snapshot);
        if (message) toast.success(message, undefined, undo ? { label: 'Undo', run: () => void runUndo(undo) } : undefined);
        return r;
      } catch (e) {
        toast.error('To-dos', errorMessage(e));
        return null;
      }
    },
    receive(snap) {
      seq++;
      setSnap(snap);
    },
    setView(v) {
      try {
        localStorage.setItem(VIEW_KEY, viewKey(v));
      } catch {
        /* private mode */
      }
      set({ view: v });
    },
    open: (id) => set({ openId: id }),
  };
});

async function runUndo(u: Undo) {
  const { apply } = useTodos.getState();
  if (u.remove?.length) await apply({ type: 'delete', ids: u.remove });
  if (u.restore?.length) await apply({ type: 'restore', tasks: u.restore });
}

/** The tasks (with sub-tasks) as they are now, for an undo. */
function capture(ids: string[]): Task[] {
  const tasks = useTodos.getState().snap?.tasks ?? [];
  const all = new Set(ids.flatMap((id) => subtree(tasks, id)));
  return tasks.filter((t) => all.has(t.id)).map(({ dueLocal: _l, dueAt: _a, ...t }) => t);
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`;

export async function completeTasks(ids: string[], done = true) {
  if (!ids.length) return;
  const before = capture(ids);
  const s = useTodos.getState();
  const r = await s.apply({ type: 'complete', ids, done });
  if (!r) return;
  const today = r.snapshot.today;
  const next = r.applied.rescheduled;
  const title = !done ? `Reopened ${ids.length === 1 ? 'task' : plural(ids.length, 'task')}` : next.length === 1 && ids.length === 1 ? `Completed — next: ${dayLabel(next[0].date, today)}` : ids.length === 1 ? 'Completed' : `Completed ${plural(ids.length, 'task')}`;
  toast.success(title, undefined, { label: 'Undo', run: () => void runUndo({ restore: before }) });
}

export async function deleteTasks(ids: string[]) {
  if (!ids.length) return;
  const before = capture(ids);
  const s = useTodos.getState();
  if (s.openId && before.some((t) => t.id === s.openId)) s.open(null);
  await s.apply({ type: 'delete', ids }, `Deleted ${plural(before.length, 'task')}`, { restore: before });
}

export async function updateTasks(ids: string[], patch: TaskPatch, message?: string) {
  if (!ids.length) return;
  const before = capture(ids);
  await useTodos.getState().apply({ type: 'update', ids, patch }, message, message ? { restore: before } : undefined);
}

export async function moveTasks(ids: string[], to: MoveTarget, message?: string) {
  if (!ids.length) return;
  const before = capture(ids);
  await useTodos.getState().apply({ type: 'move', ids, listId: to.listId, sectionId: to.sectionId, parentId: to.parentId, index: to.index }, message, message ? { restore: before } : undefined);
}

/** Forgets an attachment (the file stays on disk until garbage collection, so Undo works). */
export async function removeAttachment(taskId: string, attachment: Attachment) {
  const before = capture([taskId]).filter((t) => t.id === taskId);
  await useTodos.getState().apply({ type: 'removeAttachment', id: taskId, attachmentId: attachment.id }, `Removed ${attachment.name}`, { restore: before });
}

function attached(r: TodoAttachResult | null) {
  if (!r) return;
  useTodos.getState().receive(r.snapshot);
  for (const e of r.errors) toast.error('Could not attach a file', e);
  if (r.added.length) toast.success(r.added.length === 1 ? `Attached ${r.added[0].name}` : `Attached ${plural(r.added.length, 'file')}`);
}

/** Attaches files picked in the native dialog. */
export async function pickAttachments(taskId: string) {
  try {
    attached(await todosApi.attachPick(useTodos.getState().root, taskId));
  } catch (e) {
    toast.error('Could not attach files', errorMessage(e));
  }
}

/** Attaches dropped or pasted files (one upload each; too-large files are skipped with a message). */
export async function attachFiles(taskId: string, files: File[]) {
  const root = useTodos.getState().root;
  for (const f of files) {
    if (f.size > MAX_ATTACHMENT_BYTES) {
      toast.error(`${f.name || 'File'} is too large`, `Attachments are limited to ${MAX_ATTACHMENT_BYTES / 1_048_576} MB.`);
      continue;
    }
    try {
      attached(await todosApi.attachBytes(root, taskId, f.name, f.type, new Uint8Array(await f.arrayBuffer())));
    } catch (e) {
      toast.error(`Could not attach ${f.name || 'the file'}`, errorMessage(e));
    }
  }
}

/** Adds a task; returns its id. */
export async function addTask(task: NewTask, quiet = false): Promise<string | null> {
  const r = await useTodos.getState().apply({ type: 'add', task });
  const id = r?.applied.created ?? null;
  if (id && !quiet) {
    const t = r!.snapshot.tasks.find((x) => x.id === id);
    const list = r!.snapshot.lists.find((l) => l.id === t?.listId);
    toast.success(`Added to ${list?.name ?? 'your to-dos'}`, t?.content, { label: 'Undo', run: () => void runUndo({ remove: [id] }) });
  }
  return id;
}

/**
 * Adds a to-do to a project's list (quick-add syntax works: `add tests tomorrow p1 @api`)
 * without switching the visible list. Used by "Add to-do about this…" in the request tree.
 */
export async function addTodoFor(root: string, text: string, links: string[]) {
  try {
    const r = await todosApi.apply(root, { type: 'add', task: { text, links } });
    const st = useTodos.getState();
    if (st.root === root) await st.refresh();
    const id = r.applied.created;
    const t = r.snapshot.tasks.find((x) => x.id === id);
    toast.success('Added to your to-dos', t?.content ?? text, id ? { label: 'Undo', run: () => void todosApi.apply(root, { type: 'delete', ids: [id] }).then(() => useTodos.getState().refresh()) } : undefined);
  } catch (e) {
    toast.error('Could not add to-do', errorMessage(e));
  }
}

// Keep the snapshot current: the open project, reminders completed from a notification, edits
// from the CLI (on focus), and the date rolling over (every few minutes).
if (typeof window !== 'undefined') {
  setTimeout(() => {
    const root = useWorkspace.getState().project?.root ?? null;
    if (!useTodos.getState().snap) void useTodos.getState().load(root);
  }, 0);
  useWorkspace.subscribe((s, prev) => {
    const root = s.project?.root ?? null;
    if (root !== (prev.project?.root ?? null) && root !== useTodos.getState().root) void useTodos.getState().load(root);
  });
  window.addEventListener('focus', () => void useTodos.getState().refresh());
  setInterval(() => void useTodos.getState().refresh(), 5 * 60_000);
  try {
    void listen('todos://changed', () => void useTodos.getState().refresh()).catch(() => {});
    void listen<string>('todos://open', async (e) => {
      useNav.getState().go('todos');
      await useTodos.getState().refresh();
      useTodos.getState().open(e.payload);
    }).catch(() => {});
  } catch {
    /* not running in Tauri */
  }
}
