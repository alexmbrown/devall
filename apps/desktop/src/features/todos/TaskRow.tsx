import { Fragment, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { AlignLeft, ArrowRightLeft, AtSign, CalendarDays, Check, ChevronDown, CopyPlus, GripVertical, Link2, ListTree, MessageSquare, MoreHorizontal, Paperclip, Pencil, Plus, Repeat, Target, Trash2 } from 'lucide-react';
import { Menu } from '@/components/ui/Menu';
import type { NewTask, TaskView } from '@/lib/api/todos';
import { addTask, completeTasks, deleteTasks, moveTasks, setStatus, updateTasks, useTodos } from '@/store/todos';
import { statusChip, statusOf } from './boardModel';
import { DuePicker } from './DuePicker';
import { QuickAddInput, type QuickAddInputHandle } from './QuickAddInput';
import { LabelPicker, MovePicker, priorityItems, statusItems } from './pickers';
import { usePopover } from './Popover';
import { colorVar, dayLabel, dropPosition, dropTarget, dueInfo, rangeSelect, ruleText, subtree, todayYmd, type Row } from './lib';
import { beginDrag, endDrag, useTodoUi } from './ui';

/** Sub-task indent per level (px). */
const INDENT = 28;

/**
 * What dropping dragged tasks on a row does: `'tree'` reorders/nests (list views in manual order),
 * `{ onDrop }` hands the ids to the view (e.g. reschedule to the row's day), none: not a target.
 */
export type RowDrop = 'tree' | { onDrop(ids: string[]): void };

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

/**
 * One task: check circle, title, meta (due, deadline, labels, counts) and hover actions.
 * `context` adds where the task lives to the meta line (`list`: "List / Section", `section`: the
 * section only, for views already grouped by list).
 */
export function TaskRow({ row, context, dueMode = 'full', drop }: { row: Row; context?: 'list' | 'section'; dueMode?: 'full' | 'time'; drop?: RowDrop }) {
  const t = row.task;
  const snap = useTodos((s) => s.snap);
  const isOpen = useTodos((s) => s.openId === t.id);
  const isSel = useTodoUi((s) => s.sel.includes(t.id));
  const isCursor = useTodoUi((s) => s.cursor === t.id);
  const editing = useTodoUi((s) => s.editing === t.id);
  const dragging = useTodoUi((s) => !!s.drag?.includes(t.id));
  const dropHere = useTodoUi((s) => (s.drop?.id === t.id ? s.drop.pos : null));
  const subComposer = useTodoUi((s) => s.composer === `sub:${t.id}`);
  const setUi = useTodoUi((s) => s.set);
  const due = usePopover();
  const labels = usePopover();
  const move = usePopover();
  const ref = useRef<HTMLDivElement>(null);
  const today = snap?.today ?? todayYmd();
  const info = dueInfo(t, today);
  const list = context === 'list' ? snap?.lists.find((l) => l.id === t.listId) : undefined;
  const section = context && t.sectionId ? snap?.sections.find((s) => s.id === t.sectionId) : undefined;
  const where = list ? `${list.name}${section ? ` / ${section.name}` : ''}` : section?.name;
  const ids = () => {
    const sel = useTodoUi.getState().sel;
    return sel.includes(t.id) ? sel : [t.id];
  };
  const indent = row.depth * INDENT;

  const click = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest('button, input, a, textarea')) return;
    ref.current?.focus();
    const ui = useTodoUi.getState();
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
  const blocked = (moving: string[]) => moving.some((m) => subtree(tasks, m).includes(t.id));
  const accepts = (moving: string[] | null): moving is string[] => !!moving && !!drop && !t.done && !(drop === 'tree' ? blocked(moving) : moving.includes(t.id));

  const menu = () => [
    { label: 'Edit', icon: <Pencil />, hint: 'E', onSelect: () => setUi({ editing: t.id }) },
    { label: 'Open details', icon: <AlignLeft />, hint: 'O', onSelect: () => useTodos.getState().open(t.id) },
    { separator: true },
    { heading: 'Priority' },
    ...priorityItems(t.priority, (p) => void updateTasks(ids(), { priority: p })),
    { separator: true },
    { heading: 'Status' },
    ...statusItems(snap?.settings.columns ?? [], snap ? statusOf(t, snap.settings.columns)?.id : undefined, (c) => void setStatus(ids(), c.id, undefined, c.done ? 'Completed' : undefined)),
    { separator: true },
    { label: 'Add sub-task', icon: <ListTree />, onSelect: () => setUi({ composer: `sub:${t.id}` }) },
    {
      label: 'Duplicate',
      icon: <CopyPlus />,
      onSelect: () =>
        void addTask(
          {
            text: t.content,
            parse: false,
            listId: t.listId,
            sectionId: t.sectionId,
            parentId: t.parentId,
            afterId: t.id,
            description: t.description,
            priority: t.priority,
            due: t.due,
            deadline: t.deadline,
            labels: t.labels,
            links: t.links,
            status: t.done ? undefined : t.status,
          },
          true,
        ),
    },
    { label: 'Move to…', icon: <ArrowRightLeft />, hint: '', onSelect: () => ref.current && move.openAt(ref.current.querySelector('.td-more') ?? ref.current) },
    { label: 'Labels…', icon: <AtSign />, onSelect: () => ref.current && labels.openAt(ref.current.querySelector('.td-more') ?? ref.current) },
    { separator: true },
    { label: ids().length > 1 ? `Delete ${ids().length} tasks` : 'Delete', icon: <Trash2 />, danger: true, hint: 'Del', onSelect: () => void deleteTasks(ids()) },
  ];

  return (
    <>
      <div
        ref={ref}
        tabIndex={-1}
        data-task={t.id}
        className={clsx('td-row', t.done && 'done', row.kids > 0 && 'has-kids', isCursor && 'cursor', isSel && 'sel', isOpen && 'open-detail', dragging && 'dragging', dropHere && (drop === 'tree' ? `drop-${dropHere}` : 'drop-into'))}
        style={{ ['--indent' as string]: `${indent}px`, ['--drop-indent' as string]: `${dropHere === 'child' ? indent + INDENT : indent}px` }}
        onClick={click}
        onDoubleClick={(e) => !(e.target as HTMLElement).closest('button') && setUi({ editing: t.id })}
        draggable={!editing && !t.done}
        onDragStart={(e) => {
          if ((e.target as HTMLElement).closest('input, textarea')) return;
          beginDrag(e, { tasks: ids() });
        }}
        onDragEnd={endDrag}
        onDragOver={(e) => {
          const ui = useTodoUi.getState();
          if (!accepts(ui.drag)) return;
          e.preventDefault();
          e.stopPropagation();
          let pos: 'before' | 'after' | 'child' = 'child';
          if (drop === 'tree') {
            const r = e.currentTarget.getBoundingClientRect();
            const pad = parseFloat(getComputedStyle(e.currentTarget).paddingLeft) || 0;
            // `left` = where a top-level row's content starts.
            pos = dropPosition({ top: r.top, height: r.height, left: r.left + pad - indent }, e.clientX, e.clientY, row.depth, INDENT);
          }
          if (ui.drop?.id !== t.id || ui.drop.pos !== pos) ui.set({ drop: { id: t.id, pos } });
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null) && useTodoUi.getState().drop?.id === t.id) setUi({ drop: null });
        }}
        onDrop={(e) => {
          const ui = useTodoUi.getState();
          const moving = ui.drag;
          const pos = ui.drop?.id === t.id ? ui.drop.pos : null;
          if (!accepts(moving)) return;
          e.preventDefault();
          e.stopPropagation();
          endDrag();
          if (drop === 'tree') {
            if (pos) void moveTasks(moving, dropTarget(tasks, t, pos, moving));
          } else drop?.onDrop(moving);
        }}
      >
        {!t.done && (
          <span className="td-grip" aria-hidden>
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
            <InlineEdit task={t} onClose={() => setUi({ editing: null })} />
          ) : (
            <span className="td-content">
              <InlineText text={t.content} />
            </span>
          )}
          {!editing && t.description?.trim() && <span className="td-desc">{t.description.trim().split('\n')[0]}</span>}
          <Meta t={t} info={info} dueMode={dueMode} today={today} kids={row.kids} open={row.open} where={where} onDue={due.toggle} />
        </div>
        {!editing && (
          <div className="td-row-actions">
            <button className="icon-btn sm" onClick={() => setUi({ editing: t.id })} title="Edit (E)" aria-label="Edit">
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
      {due.anchor && <DuePicker anchor={due.anchor} onClose={due.close} mode="due" due={t.due} today={today} onPatch={(p) => void updateTasks(ids(), p)} />}
      {labels.anchor && (
        <LabelPicker anchor={labels.anchor} onClose={labels.close} selected={t.labels ?? []} onToggle={(name, on) => void updateTasks(ids(), on ? { addLabels: [name] } : { removeLabels: [name] })} />
      )}
      {move.anchor && <MovePicker anchor={move.anchor} onClose={move.close} current={{ listId: t.listId, sectionId: t.sectionId }} onPick={(listId, sectionId) => void moveTasks(ids(), { listId, sectionId, parentId: null, index: null }, `Moved to ${snap?.lists.find((l) => l.id === listId)?.name ?? 'list'}`)} />}
      {subComposer && (
        <div className="td-sub-composer" style={{ ['--indent' as string]: `${indent + INDENT}px` }}>
          <Composer defaults={{ parentId: t.id }} onDone={() => setUi({ composer: null })} placeholder="Sub-task" />
        </div>
      )}
    </>
  );
}

/**
 * The meta line under a task title (rows and board cards): due, deadline, counts, labels and where
 * the task lives. Items shrink and ellipsize instead of overflowing the row.
 */
export function Meta({
  t,
  info,
  dueMode = 'full',
  today,
  kids,
  open,
  where,
  onDue,
  hideStatus,
}: {
  t: TaskView;
  info: ReturnType<typeof dueInfo>;
  dueMode?: 'full' | 'time';
  today: string;
  kids: number;
  open: number;
  where?: string;
  onDue?(e: React.MouseEvent): void;
  /** Cards in a status column don't repeat their column. */
  hideStatus?: boolean;
}) {
  const labels = useTodos((s) => s.snap?.labels);
  const columns = useTodos((s) => s.snap?.settings.columns);
  const items: ReactNode[] = [];
  // Where it is on the board, for columns between the first and the done one ("In progress").
  const status = !hideStatus && columns ? statusChip(t, columns) : undefined;
  if (status)
    items.push(
      <span key="status" className="td-status" style={{ ['--c' as string]: colorVar(status.color) }} title={`Status: ${status.name}`}>
        <span className="truncate">{status.name}</span>
      </span>,
    );
  if (info && (dueMode === 'full' || info.time || info.recurring)) {
    const text = `${dueMode === 'full' ? info.text : ''}${info.time ? `${dueMode === 'full' ? ' ' : ''}${info.time}` : ''}`;
    items.push(
      <button key="due" className={clsx('td-due', info.late ? 'late' : info.tone)} onClick={onDue} disabled={!onDue} title={t.due?.recurrence ? ruleText(t.due) : 'Change the date'}>
        {info.recurring ? <Repeat /> : <CalendarDays />}
        {text && <span className="truncate">{text}</span>}
      </button>,
    );
  }
  if (t.deadline)
    items.push(
      <span key="dl" className={clsx('td-deadline', t.deadline < today && !t.done && 'overdue')} title="Deadline">
        <Target />
        <span className="truncate">{dayLabel(t.deadline, today)}</span>
      </span>,
    );
  if (kids > 0)
    items.push(
      <span key="kids" title="Sub-tasks done">
        <ListTree />
        {kids - open}/{kids}
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
  if (t.attachments?.length)
    items.push(
      <span key="a" title={t.attachments.map((a) => a.name).join('\n')}>
        <Paperclip />
        {t.attachments.length}
      </span>,
    );
  for (const l of t.labels ?? []) {
    const c = labels?.find((x) => x.name === l)?.color;
    items.push(
      <span key={`@${l}`} className="td-label" style={{ ['--c' as string]: colorVar(c) }} title={`@${l}`}>
        <AtSign />
        <span className="truncate">{l}</span>
      </span>,
    );
  }
  if (where)
    items.push(
      <span key="list" className="td-listname" title={where}>
        <span className="truncate">{where}</span>
      </span>,
    );
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

/** "+ Add task" that opens a composer for `slot`. With `onDropTasks` it also takes dragged tasks (end of the slot). */
export function AddTask({ slot, defaults, label = 'Add task', onDropTasks, className }: { slot: string; defaults: Omit<NewTask, 'text'>; label?: string; onDropTasks?(ids: string[]): void; className?: string }) {
  const open = useTodoUi((s) => s.composer === slot);
  const set = useTodoUi((s) => s.set);
  const [over, setOver] = useState(false);
  if (open) return <Composer defaults={defaults} onDone={() => set({ composer: null })} />;
  return (
    <button
      className={clsx('td-add-btn', over && 'drop', className)}
      onClick={() => set({ composer: slot })}
      onDragOver={(e) => {
        if (!onDropTasks || !useTodoUi.getState().drag) return;
        e.preventDefault();
        e.stopPropagation();
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        const ids = useTodoUi.getState().drag;
        setOver(false);
        if (!onDropTasks || !ids?.length) return;
        e.preventDefault();
        e.stopPropagation();
        endDrag();
        onDropTasks(ids);
      }}
    >
      <Plus /> {over ? 'Move here' : label}
    </button>
  );
}
