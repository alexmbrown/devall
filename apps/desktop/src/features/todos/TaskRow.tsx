import { Fragment, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { AlignLeft, ArrowRightLeft, AtSign, CalendarDays, Check, ChevronDown, CopyPlus, GripVertical, Link2, ListTree, MessageSquare, MoreHorizontal, Pencil, Plus, Repeat, Target, Trash2 } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import type { NewTask, TaskView } from '@/lib/api/todos';
import { addTask, completeTasks, deleteTasks, moveTasks, updateTasks, useTodos } from '@/store/todos';
import { DuePicker } from './DuePicker';
import { QuickAddInput, type QuickAddInputHandle } from './QuickAddInput';
import { LabelPicker, MovePicker, priorityItems } from './pickers';
import { usePopover } from './Popover';
import { colorVar, dayLabel, dropPosition, dropTarget, dueInfo, rangeSelect, ruleText, subtree, todayYmd, type Row } from './lib';
import { useTodoUi } from './ui';

/** Inline Markdown for titles: `code` and **bold**. */
export function InlineText({ text }: { text: string }) {
  const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith('`') && p.endsWith('`') && p.length > 2 ? (
          <code key={i}>{p.slice(1, -1)}</code>
        ) : p.startsWith('**') && p.endsWith('**') && p.length > 4 ? (
          <strong key={i}>{p.slice(2, -2)}</strong>
        ) : (
          <Fragment key={i}>{p}</Fragment>
        ),
      )}
    </>
  );
}

export function CheckCircle({ task, className }: { task: TaskView; className?: string }) {
  return (
    <button
      role="checkbox"
      aria-checked={!!task.done}
      className={clsx('td-check', `p${task.priority}`, task.done && 'on', task.due?.recurrence && 'recurring', className)}
      title={task.done ? 'Reopen' : task.due?.recurrence ? `Complete (repeats: ${ruleText(task.due)})` : 'Complete'}
      onClick={(e) => {
        e.stopPropagation();
        void completeTasks([task.id], !task.done);
      }}
    >
      <Check />
    </button>
  );
}

function InlineEdit({ task, onClose }: { task: TaskView; onClose(): void }) {
  const [text, setText] = useState(task.content);
  const [ignore, setIgnore] = useState<string[]>([]);
  const done = useRef(false);
  const save = async () => {
    if (done.current) return;
    done.current = true;
    if (text.trim() && text.trim() !== task.content) await useTodos.getState().apply({ type: 'edit', id: task.id, text, ignore });
    onClose();
  };
  return (
    <div className="td-inline-edit" onClick={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
      <QuickAddInput value={text} onChange={setText} ignore={ignore} onIgnore={setIgnore} listId={task.listId} autoFocus onSubmit={save} onBlur={save} onCancel={() => ((done.current = true), onClose())} />
    </div>
  );
}

/** One task: check circle, title, meta (due, deadline, labels, counts) and hover actions. */
export function TaskRow({ row, showList, dueMode = 'full' }: { row: Row; showList?: boolean; dueMode?: 'full' | 'time' }) {
  const t = row.task;
  const snap = useTodos((s) => s.snap);
  const openId = useTodos((s) => s.openId);
  const ui = useTodoUi();
  const due = usePopover();
  const labels = usePopover();
  const move = usePopover();
  const ref = useRef<HTMLDivElement>(null);
  const today = snap?.today ?? todayYmd();
  const info = dueInfo(t, today);
  const list = showList ? snap?.lists.find((l) => l.id === t.listId) : undefined;
  const section = showList && t.sectionId ? snap?.sections.find((s) => s.id === t.sectionId) : undefined;
  const ids = ui.sel.includes(t.id) ? ui.sel : [t.id];
  const editing = ui.editing === t.id;
  const dropHere = ui.drop?.id === t.id ? ui.drop.pos : null;
  const indent = row.depth * 28;

  const click = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('button, input, a, textarea')) return;
    ref.current?.focus();
    if (e.ctrlKey || e.metaKey) {
      const sel = ui.sel.includes(t.id) ? ui.sel.filter((x) => x !== t.id) : [...ui.sel, t.id];
      ui.set({ sel, cursor: t.id, anchor: t.id });
    } else if (e.shiftKey) {
      ui.set({ sel: rangeSelect(ui.order, ui.anchor ?? ui.cursor, t.id), cursor: t.id });
    } else {
      ui.set({ sel: [], cursor: t.id, anchor: t.id });
      useTodos.getState().open(t.id);
    }
  };

  const tasks = snap?.tasks ?? [];
  const dragging = ui.drag?.includes(t.id) ?? false;
  const blocked = (moving: string[]) => moving.some((m) => subtree(tasks, m).includes(t.id));

  const menu = () => [
    { label: 'Edit', icon: <Pencil />, hint: 'E', onSelect: () => ui.set({ editing: t.id }) },
    { label: 'Open details', icon: <AlignLeft />, hint: 'O', onSelect: () => useTodos.getState().open(t.id) },
    { separator: true },
    { heading: 'Priority' },
    ...priorityItems(t.priority, (p) => void updateTasks(ids, { priority: p })),
    { separator: true },
    { label: 'Add sub-task', icon: <ListTree />, onSelect: () => ui.set({ composer: `sub:${t.id}` }) },
    {
      label: 'Duplicate',
      icon: <CopyPlus />,
      onSelect: () =>
        void addTask(
          { text: t.content, parse: false, listId: t.listId, sectionId: t.sectionId, parentId: t.parentId, afterId: t.id, description: t.description, priority: t.priority, due: t.due, deadline: t.deadline, labels: t.labels, links: t.links },
          true,
        ),
    },
    { label: 'Move to…', icon: <ArrowRightLeft />, hint: '', onSelect: () => ref.current && move.openAt(ref.current.querySelector('.td-more') ?? ref.current) },
    { label: 'Labels…', icon: <AtSign />, onSelect: () => ref.current && labels.openAt(ref.current.querySelector('.td-more') ?? ref.current) },
    { separator: true },
    { label: ids.length > 1 ? `Delete ${ids.length} tasks` : 'Delete', icon: <Trash2 />, danger: true, hint: 'Del', onSelect: () => void deleteTasks(ids) },
  ];

  return (
    <>
      <div
        ref={ref}
        tabIndex={-1}
        data-task={t.id}
        className={clsx('td-row', t.done && 'done', ui.cursor === t.id && 'cursor', ui.sel.includes(t.id) && 'sel', openId === t.id && 'open-detail', dragging && 'dragging', dropHere && `drop-${dropHere}`)}
        style={{ paddingLeft: indent, ['--drop-indent' as string]: `${dropHere === 'child' ? indent + 28 : indent}px` }}
        onClick={click}
        onDoubleClick={(e) => !(e.target as HTMLElement).closest('button') && ui.set({ editing: t.id })}
        draggable={!editing && !t.done}
        onDragStart={(e) => {
          const moving = ui.sel.includes(t.id) ? ui.sel : [t.id];
          e.dataTransfer.effectAllowed = 'move';
          e.dataTransfer.setData('text/plain', moving.join(','));
          ui.set({ drag: moving });
        }}
        onDragEnd={() => ui.set({ drag: null, drop: null })}
        onDragOver={(e) => {
          const moving = useTodoUi.getState().drag;
          if (!moving || t.done || blocked(moving)) return;
          e.preventDefault();
          const r = e.currentTarget.getBoundingClientRect();
          const pos = dropPosition({ top: r.top, height: r.height, left: r.left }, e.clientX, e.clientY, row.depth, 28);
          if (ui.drop?.id !== t.id || ui.drop.pos !== pos) ui.set({ drop: { id: t.id, pos } });
        }}
        onDrop={(e) => {
          e.preventDefault();
          const moving = useTodoUi.getState().drag;
          const pos = useTodoUi.getState().drop?.pos;
          ui.set({ drag: null, drop: null });
          if (!moving || !pos || blocked(moving)) return;
          void moveTasks(moving, dropTarget(tasks, t, pos, moving));
        }}
      >
        {!t.done && (
          <span className="td-grip" style={{ left: indent - 18 }} aria-hidden>
            <GripVertical />
          </span>
        )}
        {row.kids > 0 ? (
          <button className={clsx('td-fold', t.collapsed && 'closed')} onClick={() => void useTodos.getState().apply({ type: 'update', ids: [t.id], patch: { collapsed: !t.collapsed } })} aria-label={t.collapsed ? 'Expand sub-tasks' : 'Collapse sub-tasks'}>
            <ChevronDown />
          </button>
        ) : null}
        <CheckCircle task={t} />
        <div className="td-main-col">
          {editing ? (
            <InlineEdit task={t} onClose={() => ui.set({ editing: null })} />
          ) : (
            <span className="td-content">
              <InlineText text={t.content} />
            </span>
          )}
          {!editing && t.description?.trim() && <span className="td-desc">{t.description.trim().split('\n')[0]}</span>}
          <Meta t={t} info={info} dueMode={dueMode} today={today} row={row} listName={list ? `${list.name}${section ? ` / ${section.name}` : ''}` : undefined} onDue={due.toggle} />
        </div>
        {!editing && (
          <div className="td-row-actions">
            <button className="icon-btn sm" onClick={() => ui.set({ editing: t.id })} title="Edit (E)" aria-label="Edit">
              <Pencil />
            </button>
            <button className="icon-btn sm" onClick={due.toggle} title="Due date (T today, M tomorrow, N no date)" aria-label="Due date">
              <CalendarDays />
            </button>
            <Menu
              align="right"
              items={menu}
              trigger={(p) => (
                <button className="icon-btn sm td-more" {...p} title="More" aria-label="More actions">
                  <MoreHorizontal />
                </button>
              )}
            />
          </div>
        )}
      </div>
      {due.anchor && <DuePicker anchor={due.anchor} onClose={due.close} mode="due" due={t.due} today={today} onPatch={(p) => void updateTasks(ids, p)} />}
      {labels.anchor && (
        <LabelPicker anchor={labels.anchor} onClose={labels.close} selected={t.labels ?? []} onToggle={(name, on) => void updateTasks(ids, on ? { addLabels: [name] } : { removeLabels: [name] })} />
      )}
      {move.anchor && <MovePicker anchor={move.anchor} onClose={move.close} current={{ listId: t.listId, sectionId: t.sectionId }} onPick={(listId, sectionId) => void moveTasks(ids, { listId, sectionId, parentId: null, index: null }, `Moved to ${snap?.lists.find((l) => l.id === listId)?.name ?? 'list'}`)} />}
      {ui.composer === `sub:${t.id}` && (
        <div style={{ paddingLeft: indent + 28 }}>
          <Composer defaults={{ parentId: t.id }} onDone={() => ui.set({ composer: null })} placeholder="Sub-task" />
        </div>
      )}
    </>
  );
}

function Meta({ t, info, dueMode, today, row, listName, onDue }: { t: TaskView; info: ReturnType<typeof dueInfo>; dueMode: 'full' | 'time'; today: string; row: Row; listName?: string; onDue(e: React.MouseEvent): void }) {
  const labels = useTodos((s) => s.snap?.labels);
  const items: ReactNode[] = [];
  if (info && (dueMode === 'full' || info.time || info.recurring)) {
    items.push(
      <button key="due" className={clsx('td-due', info.late ? 'late' : info.tone)} onClick={onDue} title={t.due?.recurrence ? ruleText(t.due) : 'Change the date'}>
        {info.recurring ? <Repeat /> : <CalendarDays />}
        {dueMode === 'full' ? info.text : ''}
        {info.time ? `${dueMode === 'full' ? ' ' : ''}${info.time}` : ''}
      </button>,
    );
  }
  if (t.deadline)
    items.push(
      <span key="dl" className={clsx('td-deadline', t.deadline < today && !t.done && 'overdue')} title="Deadline">
        <Target />
        {dayLabel(t.deadline, today)}
      </span>,
    );
  if (row.kids > 0)
    items.push(
      <span key="kids" title="Sub-tasks done">
        <ListTree />
        {row.kids - row.open}/{row.kids}
      </span>,
    );
  if (t.comments?.length)
    items.push(
      <span key="c" title="Comments">
        <MessageSquare />
        {t.comments.length}
      </span>,
    );
  if (t.links?.length)
    items.push(
      <span key="l" title={t.links.join('\n')}>
        <Link2 />
        {t.links.length}
      </span>,
    );
  for (const l of t.labels ?? []) {
    const c = labels?.find((x) => x.name === l)?.color;
    items.push(
      <span key={`@${l}`} className="td-label" style={{ ['--c' as string]: colorVar(c) }}>
        <AtSign />
        {l}
      </span>,
    );
  }
  if (listName) items.push(<span key="list" className="td-listname">{listName}</span>);
  return items.length ? <div className="td-meta">{items}</div> : null;
}

/** Inline task composer (quick-add syntax, optional description). Stays open after adding. */
export function Composer({ defaults, onDone, placeholder }: { defaults: Omit<NewTask, 'text'>; onDone(): void; placeholder?: string }) {
  const snap = useTodos((s) => s.snap);
  const [text, setText] = useState('');
  const [ignore, setIgnore] = useState<string[]>([]);
  const [desc, setDesc] = useState('');
  const input = useRef<QuickAddInputHandle>(null);
  const list = snap?.lists.find((l) => l.id === (defaults.listId ?? snap?.projectList ?? 'inbox'));
  const section = defaults.sectionId ? snap?.sections.find((s) => s.id === defaults.sectionId) : undefined;
  const parent = defaults.parentId ? snap?.tasks.find((x) => x.id === defaults.parentId) : undefined;
  const submit = async () => {
    if (!text.trim()) return;
    const id = await addTask({ ...defaults, text, ignore, description: desc.trim() || undefined }, true);
    if (id) {
      setText('');
      setIgnore([]);
      setDesc('');
      input.current?.focus();
    }
  };
  return (
    <div className="td-composer" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
      <QuickAddInput ref={input} value={text} onChange={setText} ignore={ignore} onIgnore={setIgnore} listId={defaults.listId ?? parent?.listId} autoFocus placeholder={placeholder ?? 'Task name — try “tomorrow 5pm #List @label p1”'} onSubmit={submit} onCancel={onDone} />
      <textarea className="textarea" rows={1} placeholder="Description (Markdown)" value={desc} onChange={(e) => setDesc(e.target.value)} onKeyDown={(e) => (e.key === 'Enter' && (e.ctrlKey || e.metaKey) ? void submit() : e.key === 'Escape' ? onDone() : undefined)} />
      <div className="td-composer-foot">
        <span className="subtle xs truncate grow">{parent ? `Sub-task of “${parent.content}”` : `→ ${list?.name ?? 'Inbox'}${section ? ` / ${section.name}` : ''}${defaults.due ? ` · ${dayLabel(defaults.due.date, snap?.today ?? todayYmd())}` : ''}`}</span>
        <button className="btn btn-sm btn-ghost" onClick={onDone}>
          Cancel
        </button>
        <button className="btn btn-sm btn-primary" disabled={!text.trim()} onClick={() => void submit()}>
          Add task
        </button>
      </div>
    </div>
  );
}

/** "+ Add task" that opens a composer for `slot`. */
export function AddTask({ slot, defaults, label = 'Add task' }: { slot: string; defaults: Omit<NewTask, 'text'>; label?: string }) {
  const open = useTodoUi((s) => s.composer === slot);
  const set = useTodoUi((s) => s.set);
  if (open) return <Composer defaults={defaults} onDone={() => set({ composer: null })} />;
  return (
    <button className="td-add-btn" onClick={() => set({ composer: slot })}>
      <Plus /> {label}
    </button>
  );
}
