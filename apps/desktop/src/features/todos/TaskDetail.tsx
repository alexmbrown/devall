import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { ArrowRightLeft, AtSign, CalendarDays, ChevronDown, ChevronUp, ExternalLink, FolderOpen, Hash, History, Inbox, Link2, ListTree, MessageSquare, Plus, Repeat, Target, Trash2, X } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Markdown } from '@/components/Markdown';
import { Menu } from '@/components/ui/Menu';
import { RelTime } from '@/components/ui/misc';
import { notesApi, type NoteNode } from '@/lib/api/notes';
import type { TaskView } from '@/lib/api/todos';
import type { TreeNode } from '@/lib/types';
import { useProjectLinkOpener } from '@/features/notes/NotesPage';
import { deleteTasks, moveTasks, updateTasks, useTodos } from '@/store/todos';
import { useWorkspace } from '@/store/workspace';
import { DuePicker } from './DuePicker';
import { QuickAddInput } from './QuickAddInput';
import { CheckCircle, Composer, InlineText } from './TaskRow';
import { LabelPicker, MovePicker, PriorityIcon, priorityItems } from './pickers';
import { usePopover } from './Popover';
import { colorVar, dayLabel, dueInfo, linkLabel, moveCursor, parseYmd, ruleText, timeLabel, todayYmd, treeRows } from './lib';
import { useTodoUi } from './ui';

function flattenRequests(n: TreeNode | null, out: string[] = []): string[] {
  if (!n) return out;
  if (n.kind === 'request') out.push(`http/${n.path}`);
  else n.children.forEach((c) => flattenRequests(c, out));
  return out;
}

function flattenNotes(n: NoteNode | null, out: string[] = []): string[] {
  if (!n) return out;
  if (n.kind === 'note') out.push(`notes/${n.path}`);
  else n.children.forEach((c) => flattenNotes(c, out));
  return out;
}

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

function Description({ task }: { task: TaskView }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(task.description ?? '');
  const openLink = useProjectLinkOpener();
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
      <textarea
        className="textarea"
        autoFocus
        rows={Math.min(16, Math.max(4, text.split('\n').length + 1))}
        value={text}
        placeholder="Description — Markdown: **bold**, `code`, - lists, [links](http/…)"
        onChange={(e) => setText(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) save();
          if (e.key === 'Escape') {
            setText(task.description ?? '');
            setEditing(false);
          }
        }}
      />
    );
  return (
    <div className={clsx('td-desc-view', !task.description?.trim() && 'placeholder')} onClick={(e) => !(e.target as HTMLElement).closest('a') && setEditing(true)} title="Click to edit">
      {task.description?.trim() ? <Markdown source={task.description} onProjectLink={(p) => openLink(p)} /> : 'Add a description (Markdown)…'}
    </div>
  );
}

function Links({ task, foreign }: { task: TaskView; foreign?: { name: string; root?: string } }) {
  const openLink = useProjectLinkOpener();
  const project = useWorkspace((s) => s.project);
  const tree = useWorkspace((s) => s.tree);
  const [adding, setAdding] = useState(false);
  const [value, setValue] = useState('');
  const [notes, setNotes] = useState<string[]>([]);
  useEffect(() => {
    if (adding && project) notesApi.tree(project.root).then((n) => setNotes(flattenNotes(n))).catch(() => setNotes([]));
  }, [adding, project]);
  const suggestions = useMemo(() => [...flattenRequests(tree), ...notes], [tree, notes]);
  const add = () => {
    const v = value.trim();
    if (v) void updateTasks([task.id], { addLinks: [v] });
    setValue('');
    setAdding(false);
  };
  const open = (l: string) => {
    if (/^https?:\/\//i.test(l)) void openUrl(l).catch(() => {});
    else openLink(l);
  };
  return (
    <div className="col-sm">
      {(task.links ?? []).map((l) => (
        <div key={l} className="td-link">
          {/^https?:/.test(l) ? <ExternalLink /> : <Link2 />}
          <button className="grow truncate" onClick={() => open(l)} title={foreign && !/^https?:/.test(l) ? `In ${foreign.name} — open that project to follow it` : l} disabled={!!foreign && !/^https?:/.test(l)}>
            {linkLabel(l)}
          </button>
          <button className="icon-btn sm" onClick={() => void updateTasks([task.id], { links: (task.links ?? []).filter((x) => x !== l) })} aria-label="Remove link" title="Remove link">
            <X />
          </button>
        </div>
      ))}
      {adding ? (
        <>
          <input
            className="input sm"
            autoFocus
            list="td-link-suggestions"
            placeholder="http/users/list.req.yaml, notes/auth.md or a URL"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onBlur={add}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === 'Enter') add();
              if (e.key === 'Escape') setAdding(false);
            }}
          />
          <datalist id="td-link-suggestions">
            {suggestions.map((s) => (
              <option key={s} value={s} />
            ))}
          </datalist>
        </>
      ) : (
        <button className={clsx('td-prop-btn placeholder')} onClick={() => setAdding(true)}>
          <Plus /> Link a request, note, mock or URL
        </button>
      )}
      {foreign?.root && (
        <button className="btn btn-sm btn-ghost" style={{ alignSelf: 'flex-start' }} onClick={() => void useWorkspace.getState().openProject(foreign.root!)}>
          <FolderOpen /> Open {foreign.name}
        </button>
      )}
    </div>
  );
}

function Comments({ task }: { task: TaskView }) {
  const [text, setText] = useState('');
  const openLink = useProjectLinkOpener();
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
          <Markdown source={c.text} onProjectLink={(p) => openLink(p)} />
        </div>
      ))}
      <textarea
        className="textarea"
        rows={2}
        placeholder="Comment (Markdown) — Ctrl+Enter to post"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) void post();
        }}
      />
      {text.trim() && (
        <button className="btn btn-sm btn-primary" style={{ alignSelf: 'flex-end' }} onClick={() => void post()}>
          Comment
        </button>
      )}
    </div>
  );
}

/** The right-hand panel for the open task. */
export function TaskDetail() {
  const snap = useTodos((s) => s.snap);
  const openId = useTodos((s) => s.openId);
  const order = useTodoUi((s) => s.order);
  const composer = useTodoUi((s) => s.composer);
  const setUi = useTodoUi((s) => s.set);
  const due = usePopover();
  const deadline = usePopover();
  const labels = usePopover();
  const move = usePopover();
  const task = snap?.tasks.find((t) => t.id === openId);
  if (!snap || !task) return null;
  const today = snap.today ?? todayYmd();
  const list = snap.lists.find((l) => l.id === task.listId);
  const section = task.sectionId ? snap.sections.find((s) => s.id === task.sectionId) : undefined;
  const parent = task.parentId ? snap.tasks.find((t) => t.id === task.parentId) : undefined;
  const kids = treeRows(
    snap.tasks.filter((t) => t.parentId === task.id),
    true,
    0,
  );
  const info = dueInfo(task, today);
  const foreign = list?.kind === 'project' && list.id !== snap.projectList ? { name: list.name, root: list.projectRoot } : undefined;
  const go = (dir: 1 | -1) => {
    const next = moveCursor(order, task.id, dir);
    if (next) {
      useTodos.getState().open(next);
      setUi({ cursor: next });
    }
  };

  return (
    <aside className="td-detail" aria-label="Task details" onKeyDown={(e) => e.key === 'Escape' && !(e.target as HTMLElement).closest('input, textarea') && useTodos.getState().open(null)}>
      <div className="td-detail-head">
        {list?.kind === 'inbox' ? <Inbox size={13} /> : <Hash size={13} style={{ color: colorVar(list?.color) }} />}
        <span className="truncate grow">
          {list?.name}
          {section ? ` / ${section.name}` : ''}
          {parent ? ` / ${parent.content}` : ''}
        </span>
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
          <button className="td-prop-btn placeholder" style={{ alignSelf: 'flex-start', marginBottom: -8 }} onClick={() => useTodos.getState().open(parent.id)}>
            <ListTree /> {parent.content}
          </button>
        )}
        <Title task={task} />
        <Description task={task} />
        <div className="td-props">
          <span className="k">Due</span>
          <span className="v">
            <button className={clsx('td-prop-btn', !info && 'placeholder')} onClick={due.toggle}>
              {info?.recurring ? <Repeat /> : <CalendarDays />}
              <span className={clsx('truncate', info && `td-due ${info.late ? 'late' : info.tone}`)}>
                {info ? `${parseYmd(task.dueLocal ?? task.due!.date).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })} (${info.text})${info.time ? ` ${info.time}` : ''}` : 'No date'}
              </span>
            </button>
            {task.due?.recurrence && <span className="subtle xs truncate">{ruleText(task.due)}</span>}
            {task.due?.timezone && <span className="subtle xs">{task.due.timezone}</span>}
          </span>
          <span className="k">Deadline</span>
          <span className="v">
            <button className={clsx('td-prop-btn', !task.deadline && 'placeholder')} onClick={deadline.toggle}>
              <Target />
              <span className={clsx(task.deadline && task.deadline < today && 'td-deadline overdue')}>{task.deadline ? `${parseYmd(task.deadline).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: task.deadline.slice(0, 4) !== today.slice(0, 4) ? 'numeric' : undefined })} (${dayLabel(task.deadline, today)})` : 'No deadline'}</span>
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
              <span key={l} className="tag" style={{ color: colorVar(snap.labels.find((x) => x.name === l)?.color) }}>
                <AtSign /> {l}
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
          <span className="k" style={{ alignSelf: 'start', paddingTop: 6 }}>
            Links
          </span>
          <span className="v">
            <Links task={task} foreign={foreign} />
          </span>
        </div>

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
          <Comments task={task} />
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
