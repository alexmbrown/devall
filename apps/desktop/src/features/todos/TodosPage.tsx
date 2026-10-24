import { useEffect, useRef, type ReactNode } from 'react';
import { ArrowRightLeft, AtSign, BellRing, CalendarDays, CalendarRange, Check, CheckCircle2, Eye, EyeOff, Filter, FolderKanban, Hash, Inbox, Keyboard, MoreHorizontal, Pencil, Plus, Search, Trash2, X } from 'lucide-react';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Spinner } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { AiAssistButton } from '@/features/ai';
import type { NewTask, Snapshot } from '@/lib/api/todos';
import { useViewLabel } from '@/store/nav';
import { addTask, completeTasks, deleteTasks, moveTasks, updateTasks, useTodos } from '@/store/todos';
import { useWorkspace } from '@/store/workspace';
import { DuePicker } from './DuePicker';
import { openQuickAdd } from './QuickAddDialog';
import { TaskDetail } from './TaskDetail';
import { TodoNav } from './TodoNav';
import { openTodoDialog, TodoDialogs } from './dialogs';
import { LabelPicker, MovePicker, priorityItems } from './pickers';
import { usePopover } from './Popover';
import { addDays, colorVar, indentTarget, keyAction, moveCursor, openCount, outdentTarget, viewKey, type ViewRef } from './lib';
import { targets, useTodoUi } from './ui';
import { CompletedView, FilterResults, LabelView, ListView, SearchResults, TodayView, UpcomingView } from './views';
import './todos.css';

/** Defaults for a task added from a view (quick add `Q`, AI suggestions). */
function defaultsFor(view: ViewRef, snap: Snapshot): Omit<NewTask, 'text'> {
  switch (view.kind) {
    case 'list':
      return { listId: view.id };
    case 'inbox':
      return { listId: 'inbox' };
    case 'today':
      return { due: { date: snap.today }, listId: snap.projectList ?? 'inbox' };
    case 'label':
      return { labels: [view.name], listId: snap.projectList ?? 'inbox' };
    default:
      return { listId: snap.projectList ?? 'inbox' };
  }
}

function Header({ view, snap }: { view: ViewRef; snap: Snapshot }) {
  const showDone = useTodoUi((s) => s.showDone);
  const search = useTodoUi((s) => s.search);
  const setUi = useTodoUi((s) => s.set);
  const apply = useTodos((s) => s.apply);
  let icon: ReactNode = <Inbox />;
  let title = 'Inbox';
  let sub: ReactNode = null;
  const extra: ReactNode[] = [];
  const menu: MenuItemDef[] = [];
  const list = view.kind === 'list' ? snap.lists.find((l) => l.id === view.id) : view.kind === 'inbox' ? snap.lists.find((l) => l.kind === 'inbox') : undefined;
  if (search.trim()) {
    icon = <Search />;
    title = `Search: ${search.trim()}`;
  } else if (list) {
    icon = list.kind === 'inbox' ? <Inbox /> : list.kind === 'project' ? <FolderKanban style={{ color: colorVar(list.color) }} /> : <Hash style={{ color: colorVar(list.color) }} />;
    title = list.name;
    const n = openCount(snap.tasks, (t) => t.listId === list.id);
    sub = `${n} open${list.kind === 'project' ? ' · personal list for this project' : ''}`;
    extra.push(
      <button key="done" className={showDone ? 'icon-btn active' : 'icon-btn'} onClick={() => setUi({ showDone: !showDone })} title={showDone ? 'Hide completed' : 'Show completed'} aria-label="Show completed">
        {showDone ? <EyeOff /> : <Eye />}
      </button>,
    );
    menu.push(
      { label: list.kind === 'user' ? 'Edit list…' : 'Colour…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'list', id: list.id }) },
      {
        label: 'Clear completed in this list…',
        icon: <CheckCircle2 />,
        onSelect: async () => {
          if (await confirmDialog({ title: 'Delete completed tasks?', message: `Completed tasks of “${list.name}” are removed for good.`, confirmText: 'Delete', danger: true }))
            void apply({ type: 'clearCompleted', listId: list.id }, 'Cleared completed tasks');
        },
      },
    );
  } else if (view.kind === 'today') {
    icon = <CalendarDays />;
    title = 'Today';
    sub = new Date().toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
  } else if (view.kind === 'upcoming') {
    icon = <CalendarRange />;
    title = 'Upcoming';
    sub = `${openCount(snap.tasks, (t) => !!t.due && (t.dueLocal ?? t.due.date) >= snap.today && (t.dueLocal ?? t.due.date) <= addDays(snap.today, 6))} in the next 7 days`;
  } else if (view.kind === 'label') {
    const l = snap.labels.find((x) => x.name === view.name);
    icon = <AtSign style={{ color: colorVar(l?.color) }} />;
    title = view.name;
    menu.push({ label: 'Edit label…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'label', name: view.name }) });
  } else if (view.kind === 'filter') {
    const f = snap.filters.find((x) => x.id === view.id);
    icon = <Filter style={{ color: colorVar(f?.color) }} />;
    title = f?.name ?? 'Filter';
    sub = <code className="mono">{f?.query}</code>;
    menu.push({ label: 'Edit filter…', icon: <Pencil />, onSelect: () => openTodoDialog({ kind: 'filter', id: view.id }) });
  } else if (view.kind === 'completed') {
    icon = <CheckCircle2 />;
    title = 'Completed';
    sub = `${snap.completedCount} completion${snap.completedCount === 1 ? '' : 's'}`;
    menu.push({
      label: 'Clear all completed…',
      icon: <Trash2 />,
      danger: true,
      onSelect: async () => {
        if (await confirmDialog({ title: 'Delete every completed task?', message: 'Recurring tasks keep their history.', confirmText: 'Delete', danger: true })) void apply({ type: 'clearCompleted' }, 'Cleared completed tasks');
      },
    });
  }
  useViewLabel(title);
  const target = defaultsFor(view, snap);
  menu.push(
    ...(menu.length ? [{ separator: true }] : []),
    { label: 'Reminders…', icon: <BellRing />, onSelect: () => openTodoDialog({ kind: 'settings' }) },
    { label: 'Keyboard shortcuts & syntax', icon: <Keyboard />, hint: '?', onSelect: () => openTodoDialog({ kind: 'help' }) },
  );
  return (
    <div className="td-head">
      <h1 className="truncate">
        {icon}
        <span className="truncate">{title}</span>
      </h1>
      {sub && <span className="td-sub truncate">{sub}</span>}
      <span className="spacer" />
      {extra}
      <AiAssistButton
        feature="todosFromText"
        align="right"
        prompt
        label="From notes…"
        title="Turn meeting notes, a ticket or an error log into to-dos (local AI)"
        placeholder="Paste meeting notes, a ticket or a log"
        input={(text) => ({ text })}
        onResult={async (r) => {
          let n = 0;
          for (const t of r.todos) {
            const id = await addTask({ ...target, text: t.text, parse: false, description: t.description || undefined, priority: t.priority, dueString: t.due ?? undefined, due: t.due ? undefined : target.due, labels: [...(target.labels ?? []), ...t.labels], links: t.links }, true);
            if (id) n++;
          }
          if (n) toast.success(`Added ${n} task${n === 1 ? '' : 's'}`);
        }}
        applyLabel="Add to-dos"
      />
      <Menu
        align="right"
        items={menu}
        trigger={(p) => (
          <button className="icon-btn" {...p} title="More" aria-label="More">
            <MoreHorizontal />
          </button>
        )}
      />
      <button className="btn btn-sm btn-primary" onClick={() => openQuickAdd({ defaults: target })} title="Quick add (Q)">
        <Plus /> Add task
      </button>
    </div>
  );
}

function BulkBar() {
  const sel = useTodoUi((s) => s.sel);
  const setUi = useTodoUi((s) => s.set);
  const snap = useTodos((s) => s.snap);
  const due = usePopover();
  const labels = usePopover();
  const move = usePopover();
  if (!snap || sel.length < 1) return null;
  const tasks = snap.tasks.filter((t) => sel.includes(t.id));
  const common = (snap.labels ?? []).map((l) => l.name).filter((n) => tasks.every((t) => t.labels?.includes(n)));
  const n = sel.length;
  return (
    <div className="td-bulk" role="toolbar" aria-label="Selected tasks">
      <span className="strong">{n} selected</span>
      <button className="btn btn-sm btn-ghost" onClick={() => void completeTasks(sel).then(() => setUi({ sel: [] }))}>
        <Check /> Complete
      </button>
      <button className="btn btn-sm btn-ghost" onClick={due.toggle}>
        <CalendarDays /> Date
      </button>
      <Menu
        items={priorityItems(undefined, (p) => void updateTasks(sel, { priority: p }, `Priority ${p} for ${n} task${n === 1 ? '' : 's'}`))}
        trigger={(p) => (
          <button className="btn btn-sm btn-ghost" {...p}>
            Priority
          </button>
        )}
      />
      <button className="btn btn-sm btn-ghost" onClick={labels.toggle}>
        <AtSign /> Labels
      </button>
      <button className="btn btn-sm btn-ghost" onClick={move.toggle}>
        <ArrowRightLeft /> Move
      </button>
      <button className="btn btn-sm btn-danger-ghost" onClick={() => void deleteTasks(sel).then(() => setUi({ sel: [] }))}>
        <Trash2 /> Delete
      </button>
      <span className="sep" />
      <button className="icon-btn sm" onClick={() => setUi({ sel: [] })} title="Clear selection (Esc)" aria-label="Clear selection">
        <X />
      </button>
      {due.anchor && <DuePicker anchor={due.anchor} onClose={due.close} mode="due" today={snap.today} onPatch={(p) => void updateTasks(sel, p, `Rescheduled ${n} task${n === 1 ? '' : 's'}`)} />}
      {labels.anchor && <LabelPicker anchor={labels.anchor} onClose={labels.close} selected={common} onToggle={(name, on) => void updateTasks(sel, on ? { addLabels: [name] } : { removeLabels: [name] })} />}
      {move.anchor && <MovePicker anchor={move.anchor} onClose={move.close} onPick={(listId, sectionId) => void moveTasks(sel, { listId, sectionId, parentId: null, index: null }, `Moved ${n} task${n === 1 ? '' : 's'}`)} />}
    </div>
  );
}

function Content({ view }: { view: ViewRef }) {
  switch (view.kind) {
    case 'inbox':
      return <ListView listId="inbox" />;
    case 'list':
      return <ListView listId={view.id} />;
    case 'today':
      return <TodayView />;
    case 'upcoming':
      return <UpcomingView />;
    case 'label':
      return <LabelView name={view.name} />;
    case 'filter':
      return <FilterResults id={view.id} />;
    case 'completed':
      return <CompletedView />;
  }
}

function focusRow(id: string | null) {
  if (!id) return;
  const el = document.querySelector<HTMLElement>(`.td-row[data-task="${CSS.escape(id)}"]`);
  el?.focus({ preventScroll: true });
  el?.scrollIntoView({ block: 'nearest' });
}

/**
 * Personal to-dos, Todoist style: Inbox, Today, Upcoming, lists with sections, labels, saved
 * filters, quick add with natural language, sub-tasks, recurring tasks and reminders. Data lives in
 * the app data folder (`todos/todos.json`), never in the repository.
 */
export default function TodosPage() {
  const project = useWorkspace((s) => s.project);
  const snap = useTodos((s) => s.snap);
  const view = useTodos((s) => s.view);
  const openId = useTodos((s) => s.openId);
  const search = useTodoUi((s) => s.search);
  const quickAdd = useTodoUi((s) => s.quickAdd);
  const page = useRef<HTMLDivElement>(null);
  const lastG = useRef(0);

  useEffect(() => {
    const root = project?.root ?? null;
    const st = useTodos.getState();
    if (root !== st.root || !st.snap) void st.load(root);
  }, [project?.root]);
  const ready = !!snap && !!view;
  useEffect(() => {
    if (ready && !page.current?.contains(document.activeElement)) page.current?.focus({ preventScroll: true });
  }, [ready]);
  const vKey = view ? viewKey(view) : '';
  useEffect(() => {
    useTodoUi.getState().set({ sel: [], cursor: null, editing: null, composer: null });
  }, [vKey, search]);
  useEffect(() => {
    if (!quickAdd) return;
    useTodoUi.getState().set({ quickAdd: false });
    const s = useTodos.getState();
    if (s.snap && s.view) openQuickAdd({ defaults: defaultsFor(s.view, s.snap) });
  }, [quickAdd]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.defaultPrevented) return;
    const el = e.target as HTMLElement;
    if (el.closest('input, textarea, select, [contenteditable="true"], .monaco-editor, .modal, .td-pop, .menu')) return;
    if (el.tagName === 'BUTTON' && (e.key === 'Enter' || e.key === ' ')) return;
    if (e.key === 'g' && !e.ctrlKey && !e.metaKey) {
      lastG.current = Date.now();
      return;
    }
    if (Date.now() - lastG.current < 1200) return;
    if (e.key === '?') {
      openTodoDialog({ kind: 'help' });
      return;
    }
    const a = keyAction(e);
    const st = useTodos.getState();
    const ui = useTodoUi.getState();
    if (!a || !st.snap || !st.view) return;
    const tasks = st.snap.tasks;
    const ids = targets();
    e.preventDefault();
    e.stopPropagation();
    switch (a) {
      case 'quickAdd':
        openQuickAdd({ defaults: defaultsFor(st.view, st.snap) });
        break;
      case 'search':
        document.getElementById('td-search')?.focus();
        break;
      case 'down':
      case 'up': {
        const next = moveCursor(ui.order, ui.cursor, a === 'down' ? 1 : -1);
        ui.set({ cursor: next, anchor: next, sel: e.shiftKey && next ? [...new Set([...(ui.sel.length ? ui.sel : ui.cursor ? [ui.cursor] : []), next])] : [] });
        focusRow(next);
        break;
      }
      case 'edit':
        if (ui.cursor) ui.set({ editing: ui.cursor });
        break;
      case 'open':
        if (ui.cursor) st.open(ui.cursor);
        break;
      case 'complete':
        void completeTasks(ids, !tasks.find((t) => t.id === ids[0])?.done);
        break;
      case 'p1':
      case 'p2':
      case 'p3':
      case 'p4':
        void updateTasks(ids, { priority: Number(a.slice(1)) as 1 | 2 | 3 | 4 });
        break;
      case 'today':
        void updateTasks(ids, { dueDate: st.snap.today }, ids.length ? 'Due today' : undefined);
        break;
      case 'tomorrow':
        void updateTasks(ids, { dueDate: addDays(st.snap.today, 1) }, ids.length ? 'Due tomorrow' : undefined);
        break;
      case 'noDate':
        void updateTasks(ids, { dueDate: '' }, ids.length ? 'Date removed' : undefined);
        break;
      case 'indent':
      case 'outdent':
        void (async () => {
          for (const id of ids) {
            const t = useTodos.getState().snap?.tasks.find((x) => x.id === id);
            const to = t && (a === 'indent' ? indentTarget(useTodos.getState().snap!.tasks, t) : outdentTarget(useTodos.getState().snap!.tasks, t));
            if (to) await moveTasks([id], to);
          }
          focusRow(ui.cursor);
        })();
        break;
      case 'delete':
        void deleteTasks(ids).then(() => ui.set({ sel: [] }));
        break;
      case 'selectAll':
        ui.set({ sel: ui.order });
        break;
      case 'escape':
        if (ui.sel.length) ui.set({ sel: [] });
        else if (ui.editing) ui.set({ editing: null });
        else if (st.openId) st.open(null);
        else ui.set({ cursor: null });
        break;
    }
  };

  if (!snap || !view) {
    return (
      <div className="page" style={{ alignItems: 'center', justifyContent: 'center' }}>
        <Spinner />
      </div>
    );
  }
  return (
    <div ref={page} className="split td-root" tabIndex={-1} onKeyDown={onKey}>
      <TodoNav />
      <div className="pane fill td-main">
        <Header view={view} snap={snap} />
        <div className="td-body">{search.trim() ? <SearchResults q={search} /> : <Content view={view} />}</div>
        <BulkBar />
      </div>
      {openId && <TaskDetail />}
      <TodoDialogs />
    </div>
  );
}
