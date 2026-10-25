import { TodoIcon } from '@/components/links/kindIcons';
import { todosApi, type Snapshot, type TaskView } from '@/lib/api/todos';
import { linkMessage, notifyLinks, type LinkItem, type LinkProvider, type Resolved } from '@/lib/links/registry';
import { formatLink } from '@/lib/links/syntax';
import { useTodos } from '@/store/todos';
import { statusChip, statusOf } from './boardModel';

/** Where a task lives: "List / Section". */
function where(t: TaskView, snap: Snapshot): string {
  const list = snap.lists.find((l) => l.id === t.listId)?.name ?? '';
  const section = t.sectionId ? snap.sections.find((s) => s.id === t.sectionId)?.name : undefined;
  return section ? `${list} / ${section}` : list;
}

/**
 * Tasks the snapshot leaves out (completed more than a week ago), fetched by id when a link
 * names them: the task, or null when this machine doesn't have it.
 */
const extra = new Map<string, TaskView | null>();
const wanted = new Set<string>();
let timer: ReturnType<typeof setTimeout> | undefined;

function fetchLater(id: string) {
  if (extra.has(id) || wanted.has(id)) return;
  wanted.add(id);
  clearTimeout(timer);
  timer = setTimeout(() => {
    const ids = [...wanted];
    wanted.clear();
    todosApi
      .get(ids)
      .then((found) => {
        for (const id of ids) extra.set(id, found.find((t) => t.id === id) ?? null);
      })
      .catch(() => ids.forEach((id) => extra.set(id, null)))
      .finally(notifyLinks);
  }, 30);
}

function describeTask(t: TaskView, snap: Snapshot): Resolved {
  const chip = statusChip(t, snap.settings.columns);
  const status = statusOf(t, snap.settings.columns);
  return {
    title: t.content,
    icon: <TodoIcon done={t.done} priority={t.priority} />,
    badge: chip ? <span className="dl-badge">{chip.name}</span> : undefined,
    detail: `${where(t, snap) || 'To-do'}${status ? ` · ${status.name}` : ''}`,
    done: !!t.done,
  };
}

/** Personal to-dos are per machine: what a link to one shows elsewhere (a teammate opening a shared note). */
const NOT_HERE = 'personal to-do — not on this machine';

/** `devall:todo/<task id>`: the user's personal to-dos (any list). */
export const todoLinks: LinkProvider = {
  kinds: ['todo'],
  items() {
    const snap = useTodos.getState().snap;
    if (!snap) return [];
    const open = snap.tasks.filter((t) => !t.done);
    const done = snap.tasks.filter((t) => t.done);
    return [...open, ...done].map<LinkItem>((t) => ({ link: formatLink('todo', t.id), kind: 'todo', title: t.content, detail: where(t, snap), keywords: (t.labels ?? []).join(' '), icon: <TodoIcon done={t.done} priority={t.priority} /> }));
  },
  resolve(_kind, id) {
    const snap = useTodos.getState().snap;
    if (!snap) return undefined;
    const t = snap.tasks.find((x) => x.id === id);
    if (t) return describeTask(t, snap);
    const old = extra.get(id);
    if (old === undefined) {
      fetchLater(id);
      return undefined;
    }
    return old ? describeTask(old, snap) : { unavailable: NOT_HERE };
  },
  async open(_kind, id, { go }) {
    const st = useTodos.getState();
    if (!st.snap?.tasks.some((t) => t.id === id)) await st.refresh();
    const snap = useTodos.getState().snap;
    if (snap?.tasks.some((t) => t.id === id)) {
      go('todos');
      useTodos.getState().open(id);
      return;
    }
    const old = extra.get(id) ?? (await todosApi.get([id]).catch(() => []))[0];
    if (old) {
      go('todos');
      useTodos.getState().setView({ kind: 'completed' });
      linkMessage(`“${old.content}” was completed`, 'Older completed tasks are listed under Completed.');
    } else linkMessage('Personal to-do — not on this machine', 'To-dos are stored on each person’s computer, so a link to one only opens where it was made.');
  },
  icon: () => <TodoIcon />,
};

useTodos.subscribe((s, prev) => {
  if (s.snap !== prev.snap) notifyLinks();
});
