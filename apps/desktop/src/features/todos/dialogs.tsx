import { useEffect, useMemo, useState } from 'react';
import { create } from 'zustand';
import { ArrowDown, ArrowUp, BellRing, Plus, Trash2 } from 'lucide-react';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Kbd, MOD, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { todosApi, type StatusColumn, type TodoSettings } from '@/lib/api/todos';
import { saveColumns, useTodos } from '@/store/todos';
import { columnCounts, columnsError, doneColumn, firstColumn } from './boardModel';
import { COLORS, colorVar, labelColor } from './lib';
import { ColorPicker, LabelChip, StatusDot } from './pickers';

type DialogSpec =
  | { kind: 'list'; id?: string }
  | { kind: 'label'; name?: string }
  | { kind: 'mergeLabel'; name: string }
  | { kind: 'filter'; id?: string; query?: string }
  | { kind: 'settings' }
  | { kind: 'help' }
  | { kind: 'columns' }
  | { kind: 'deleteColumn'; id: string };

const useDialog = create<{ spec: DialogSpec | null }>(() => ({ spec: null }));

export function openTodoDialog(spec: DialogSpec) {
  useDialog.setState({ spec });
}

const close = () => useDialog.setState({ spec: null });

function ListDialog({ id }: { id?: string }) {
  const list = useTodos((s) => s.snap?.lists.find((l) => l.id === id));
  const [name, setName] = useState(list?.name ?? '');
  const [color, setColor] = useState<string | null>(list?.color ?? null);
  const editableName = !list || list.kind === 'user';
  const save = async () => {
    if (editableName && !name.trim()) return;
    const r = await useTodos.getState().apply({ type: 'saveList', id, name: name.trim(), color });
    if (r) {
      close();
      if (!id && r.applied.created) useTodos.getState().setView({ kind: 'list', id: r.applied.created });
    }
  };
  return (
    <Modal
      open
      onClose={close}
      title={id ? `Edit ${list?.name ?? 'list'}` : 'New list'}
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={editableName && !name.trim()}>
            {id ? 'Save' : 'Create list'}
          </button>
        </>
      }
    >
      <div className="col">
        {editableName ? (
          <Field label="Name" hint="Lists are personal, like Todoist projects. Add tasks to one with #Name in quick add.">
            <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void save()} placeholder="e.g. Side project" />
          </Field>
        ) : (
          <p className="small muted">{list?.kind === 'inbox' ? 'The Inbox is not tied to any project.' : 'This list belongs to a /dev/all project and is named after it.'}</p>
        )}
        <Field label="Colour">
          <ColorPicker value={color} onChange={setColor} />
        </Field>
      </div>
    </Modal>
  );
}

/** After a label is renamed or merged away: a view showing it follows. */
function followLabel(from: string, to: string) {
  const v = useTodos.getState().view;
  if (v?.kind === 'label' && v.name === from) useTodos.getState().setView({ kind: 'label', name: to });
}

function LabelDialog({ name: initial }: { name?: string }) {
  const labels = useTodos((s) => s.snap?.labels);
  const label = labels?.find((l) => l.name === initial);
  const [name, setName] = useState(initial ?? '');
  const [color, setColor] = useState<string | null>(label?.color ?? null);
  const clean = name.trim().replace(/^@/, '');
  const valid = clean && !/\s/.test(clean);
  // Renaming onto another label: offer to merge them.
  const other = initial ? labels?.find((l) => l.name.toLowerCase() === clean.toLowerCase() && l.name !== initial) : undefined;
  const save = async () => {
    if (!valid) return;
    if (other && initial) {
      const r = await useTodos.getState().apply({ type: 'mergeLabel', name: initial, into: other.name }, `Merged @${initial} into @${other.name}`);
      if (r) {
        close();
        followLabel(initial, other.name);
      }
      return;
    }
    const r = await useTodos.getState().apply(initial ? { type: 'saveLabel', name: initial, newName: clean, color } : { type: 'saveLabel', name: clean, color });
    if (r) {
      close();
      if (initial) followLabel(initial, clean);
    }
  };
  return (
    <Modal
      open
      onClose={close}
      title={initial ? `Edit @${initial}` : 'New label'}
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={!valid}>
            {other ? `Merge into @${other.name}` : initial ? 'Save' : 'Create label'}
          </button>
        </>
      }
    >
      <div className="col">
        <Field
          label="Name"
          error={clean && !valid ? 'One word (use - or _)' : undefined}
          hint={other ? `@${other.name} exists: saving merges @${initial} into it (its tasks get @${other.name}).` : 'Type @name in quick add to label a task.'}
        >
          <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void save()} placeholder="e.g. waiting-on" />
        </Field>
        {!other && (
          <Field label="Colour" hint="Without a colour of its own, a label takes one from its name.">
            <ColorPicker value={color} onChange={setColor} custom auto={labelColor(clean || 'label', [])} noneLabel="Default colour (from the name)" />
          </Field>
        )}
        {clean && valid && !other && (
          <div className="row-sm">
            <span className="subtle xs">Preview</span>
            <LabelChip name={clean} color={color ?? labelColor(clean, [])} />
          </div>
        )}
      </div>
    </Modal>
  );
}

/** Merges a label into another: its tasks get the other label, and it goes away. */
function MergeLabelDialog({ name }: { name: string }) {
  const labels = useTodos((s) => s.snap?.labels ?? []);
  const tasks = useTodos((s) => s.snap?.tasks ?? []);
  const others = labels.filter((l) => l.name !== name);
  const [into, setInto] = useState(others[0]?.name ?? '');
  const n = tasks.filter((t) => t.labels?.includes(name)).length;
  const save = async () => {
    if (!into) return;
    const r = await useTodos.getState().apply({ type: 'mergeLabel', name, into }, `Merged @${name} into @${into}`);
    if (r) {
      close();
      followLabel(name, into);
    }
  };
  return (
    <Modal
      open
      onClose={close}
      title={`Merge @${name}`}
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={!into}>
            Merge
          </button>
        </>
      }
    >
      <div className="col">
        {others.length ? (
          <Field label="Into" hint={`${n} loaded task${n === 1 ? '' : 's'} with @${name} get the label you pick; @${name} is removed.`}>
            <select className="select" value={into} onChange={(e) => setInto(e.target.value)} autoFocus>
              {others.map((l) => (
                <option key={l.name} value={l.name}>
                  @{l.name}
                </option>
              ))}
            </select>
          </Field>
        ) : (
          <p className="small muted">There's no other label to merge into.</p>
        )}
        {into && (
          <div className="row-sm">
            <LabelChip name={name} color={labelColor(name, labels)} /> → <LabelChip name={into} color={labelColor(into, labels)} />
          </div>
        )}
      </div>
    </Modal>
  );
}

export const FILTER_SYNTAX: [string, string][] = [
  ['today | overdue', 'due today or earlier'],
  ['p1 & @bug', 'priority 1 with the label bug'],
  ['#Work & !@later', 'in the list Work, without @later'],
  ['due before: next week', 'also due:, due after:, deadline before:'],
  ['7 days', 'due in the next 7 days (with today)'],
  ['no date', 'also no labels, no deadline, recurring, subtask'],
  ['search: invoice', 'title or description contains'],
  ['status: in progress', 'board column; !status: to do excludes one'],
  ['has: attachments', 'also links, comments, subtasks, description, deadline'],
  ['@review*  /back*', 'wildcards; /section'],
  ['overdue, today', 'comma: separate groups'],
];

function FilterDialog({ id, query: initialQuery }: { id?: string; query?: string }) {
  const f = useTodos((s) => s.snap?.filters.find((x) => x.id === id));
  const [name, setName] = useState(f?.name ?? '');
  const [query, setQuery] = useState(f?.query ?? initialQuery ?? '');
  const [color, setColor] = useState<string | null>(f?.color ?? null);
  const [preview, setPreview] = useState<{ n: number; error?: string } | null>(null);
  useEffect(() => {
    if (!query.trim()) return setPreview(null);
    const t = setTimeout(() => {
      todosApi
        .filter(query)
        .then((g) => setPreview({ n: g.reduce((a, x) => a + x.ids.length, 0) }))
        .catch((e) => setPreview({ n: 0, error: errorMessage(e) }));
    }, 150);
    return () => clearTimeout(t);
  }, [query]);
  const save = async () => {
    if (!query.trim() || preview?.error) return;
    const r = await useTodos.getState().apply({ type: 'saveFilter', id, name: name.trim(), query, color });
    if (r) {
      close();
      const created = r.applied.created;
      if (created) useTodos.getState().setView({ kind: 'filter', id: created });
    }
  };
  return (
    <Modal
      open
      onClose={close}
      title={id ? `Edit filter` : 'New filter'}
      size="lg"
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={!query.trim() || !!preview?.error}>
            {id ? 'Save' : 'Create filter'}
          </button>
        </>
      }
    >
      <div className="col">
        <Field label="Name">
          <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Urgent this week" />
        </Field>
        <Field label="Query" error={preview?.error} hint={preview && !preview.error ? `${preview.n} open task${preview.n === 1 ? '' : 's'} match` : undefined}>
          <input className="input mono" value={query} onChange={(e) => setQuery(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void save()} placeholder="(p1 | p2) & 7 days" />
        </Field>
        <div className="td-syntax">
          {FILTER_SYNTAX.map(([q, d]) => (
            <FragmentRow key={q} q={q} d={d} onUse={() => setQuery(q)} />
          ))}
        </div>
        <Field label="Colour">
          <ColorPicker value={color} onChange={setColor} />
        </Field>
      </div>
    </Modal>
  );
}

function FragmentRow({ q, d, onUse }: { q: string; d: string; onUse(): void }) {
  return (
    <>
      <button className="btn btn-sm btn-ghost mono" style={{ justifyContent: 'flex-start' }} onClick={onUse} title="Use this">
        {q}
      </button>
      <span className="subtle" style={{ alignSelf: 'center' }}>
        {d}
      </span>
    </>
  );
}

function SettingsDialog() {
  const current = useTodos((s) => s.snap?.settings);
  const [s, setS] = useState<TodoSettings>(current ?? { reminders: true, remindBeforeMinutes: 0, columns: [] });
  const save = async () => {
    const r = await useTodos.getState().apply({ type: 'saveSettings', settings: { ...s, allDayReminder: s.allDayReminder || undefined } }, 'Reminder settings saved');
    if (r) close();
  };
  return (
    <Modal
      open
      onClose={close}
      title="Reminders"
      footer={
        <>
          <button
            className="btn"
            onClick={() =>
              todosApi
                .notifyTest()
                .then(() => toast.info('Test notification sent', 'Nothing showed up? Check your system notification settings for /dev/all.'))
                .catch((e) => toast.error('Notifications unavailable', errorMessage(e)))
            }
          >
            <BellRing /> Send a test
          </button>
          <span className="spacer" />
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()}>
            Save
          </button>
        </>
      }
    >
      <div className="col">
        <Switch checked={s.reminders} onChange={(v) => setS({ ...s, reminders: v })} label="Desktop notification when a task with a time is due" />
        <Field label="Also remind me before">
          <select className="select" value={s.remindBeforeMinutes} disabled={!s.reminders} onChange={(e) => setS({ ...s, remindBeforeMinutes: Number(e.target.value) })}>
            {[0, 5, 10, 15, 30, 60, 120].map((m) => (
              <option key={m} value={m}>
                {m === 0 ? 'No early reminder' : m < 60 ? `${m} minutes before` : `${m / 60} hour${m > 60 ? 's' : ''} before`}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Tasks without a time" hint="A reminder at this time on the day they're due.">
          <div className="row-sm">
            <Switch checked={!!s.allDayReminder} disabled={!s.reminders} onChange={(v) => setS({ ...s, allDayReminder: v ? (s.allDayReminder ?? '09:00') : undefined })} />
            <input type="time" className="input sm" style={{ width: 120 }} disabled={!s.reminders || !s.allDayReminder} value={s.allDayReminder ?? '09:00'} onChange={(e) => setS({ ...s, allDayReminder: e.target.value })} />
          </div>
        </Field>
        <Callout>Reminders fire only while /dev/all is running (nothing runs in the background after you quit). Clicking one opens the task; its Complete button completes it.</Callout>
      </div>
    </Modal>
  );
}

type ColumnRow = StatusColumn & { key: string };

const colourName = (c: string) => c[0].toUpperCase() + c.slice(1);

function colourItems(current: string | null | undefined, onPick: (c: string | null) => void): MenuItemDef[] {
  return [
    { label: 'No colour', icon: <span className="td-swatch" />, hint: !current ? '✓' : undefined, onSelect: () => onPick(null) },
    ...COLORS.map((c) => ({ label: colourName(c), icon: <span className="td-swatch" style={{ background: colorVar(c) }} />, hint: current === c ? '✓' : undefined, onSelect: () => onPick(c) })),
  ];
}

/**
 * The board's status columns: rename, colour, WIP limit, order, which one is the done column, add
 * and delete (asking where the tasks of a deleted column go). Saved in one `saveColumns`.
 */
function ColumnsDialog() {
  const snap = useTodos((s) => s.snap);
  const initial = useMemo(() => snap?.settings.columns ?? [], [snap]);
  const [rows, setRows] = useState<ColumnRow[]>(() => initial.map((c) => ({ ...c, key: c.id })));
  const [moves, setMoves] = useState<Record<string, string>>({});
  const [focus, setFocus] = useState<string | null>(null);
  const counts = useMemo(() => columnCounts(snap?.tasks ?? [], initial), [snap, initial]);
  const removed = initial.filter((c) => !rows.some((r) => r.id === c.id) && (counts.get(c.id) ?? 0) > 0);
  // Tasks can only go to columns that exist already.
  const targets = rows.filter((r) => r.id);
  const first = firstColumn(rows);
  const error = columnsError(rows);
  const oldDone = doneColumn(initial);
  const newDone = doneColumn(rows);
  const doneMoves = newDone && newDone.id !== oldDone?.id ? (counts.get(newDone.id) ?? 0) : 0;
  const patch = (key: string, p: Partial<ColumnRow>) => setRows((rs) => rs.map((r) => (r.key === key ? { ...r, ...p } : r)));
  const move = (i: number, delta: -1 | 1) =>
    setRows((rs) => {
      const j = i + delta;
      if (j < 0 || j >= rs.length) return rs;
      const out = [...rs];
      [out[i], out[j]] = [out[j], out[i]];
      return out;
    });
  const add = () => {
    const key = `new-${Date.now()}`;
    setRows((rs) => {
      const at = rs.length && rs[rs.length - 1].done ? rs.length - 1 : rs.length;
      return [...rs.slice(0, at), { key, id: '', name: '' }, ...rs.slice(at)];
    });
    setFocus(key);
  };
  const save = async () => {
    if (error) return;
    const reassign: Record<string, string> = {};
    for (const c of removed) if (moves[c.id] && targets.some((t) => t.id === moves[c.id])) reassign[c.id] = moves[c.id];
    const r = await saveColumns(
      rows.map(({ key: _key, ...c }) => ({ ...c, name: c.name.trim() })),
      reassign,
      'Columns saved',
    );
    if (r) close();
  };
  return (
    <Modal
      open
      onClose={close}
      title="Board columns"
      size="lg"
      footer={
        <>
          <button className="btn" onClick={add} disabled={rows.length >= 20}>
            <Plus /> Add column
          </button>
          <span className="spacer" />
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={() => void save()} disabled={!!error}>
            Save
          </button>
        </>
      }
    >
      <div className="col">
        <p className="small muted">
          Every board (lists, All tasks, Today, Upcoming, labels and filters) shows these columns. New tasks start in the first column; moving a card to the done column completes the task, and
          reopening it brings it back.
        </p>
        <div className="td-col-editor" role="list">
          {rows.map((r, i) => (
            <div key={r.key} className="td-col-row" role="listitem">
              <span className="td-col-moves">
                <button className="icon-btn sm" onClick={() => move(i, -1)} disabled={i === 0} aria-label={`Move ${r.name || 'column'} up`} title="Move up (left on the board)">
                  <ArrowUp />
                </button>
                <button className="icon-btn sm" onClick={() => move(i, 1)} disabled={i === rows.length - 1} aria-label={`Move ${r.name || 'column'} down`} title="Move down (right on the board)">
                  <ArrowDown />
                </button>
              </span>
              <Menu
                items={() => colourItems(r.color, (color) => patch(r.key, { color }))}
                trigger={(p) => (
                  <button className="icon-btn sm" {...p} aria-label="Colour" title="Colour">
                    <StatusDot col={r} />
                  </button>
                )}
              />
              <input
                className="input sm td-col-name"
                autoFocus={focus === r.key}
                value={r.name}
                placeholder="Column name"
                maxLength={40}
                aria-label="Column name"
                onChange={(e) => patch(r.key, { name: e.target.value })}
                onKeyDown={(e) => e.key === 'Enter' && void save()}
              />
              <span className="subtle xs td-col-count" title="Open tasks in this column">
                {r.id && counts.get(r.id) ? `${counts.get(r.id)} open` : ''}
              </span>
              <input
                className="input sm td-col-wip"
                type="number"
                min={0}
                max={9999}
                placeholder={r.done ? '—' : 'No limit'}
                disabled={!!r.done}
                aria-label="WIP limit"
                title="WIP limit: most open cards in this column"
                value={r.done ? '' : (r.wipLimit ?? '')}
                onChange={(e) => patch(r.key, { wipLimit: Math.max(0, Math.floor(Number(e.target.value))) || null })}
              />
              <label className="td-col-done" title="The done column: moving a card here completes the task">
                <input type="radio" name="td-done-column" checked={!!r.done} onChange={() => setRows((rs) => rs.map((x) => ({ ...x, done: x.key === r.key, wipLimit: x.key === r.key ? null : x.wipLimit })))} />
                Done
              </label>
              <button className="icon-btn sm danger" onClick={() => setRows((rs) => rs.filter((x) => x.key !== r.key))} disabled={!!r.done} aria-label={`Delete ${r.name || 'column'}`} title={r.done ? 'Make another column the done column first' : 'Delete column'}>
                <Trash2 />
              </button>
            </div>
          ))}
        </div>
        {removed.map((c) => (
          <Field key={c.id} label={`${counts.get(c.id)} open task${counts.get(c.id) === 1 ? '' : 's'} of “${c.name}” move to`}>
            <select className="select" value={moves[c.id] ?? ''} onChange={(e) => setMoves({ ...moves, [c.id]: e.target.value })}>
              <option value="">{`The first column${first?.name.trim() ? ` (${first.name.trim()})` : ''}`}</option>
              {targets.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name.trim() || t.id}
                  {t.done ? ' — completes them' : ''}
                </option>
              ))}
            </select>
          </Field>
        ))}
        {doneMoves > 0 && newDone && <Callout kind="warning">{`${doneMoves} open task${doneMoves === 1 ? '' : 's'} in “${newDone.name}” will move to the first column (completed tasks always show in the done column).`}</Callout>}
        {error && <div className="td-col-error small">{error}</div>}
      </div>
    </Modal>
  );
}

/** Deleting one column from its header: where do its tasks go? */
function DeleteColumnDialog({ id }: { id: string }) {
  const snap = useTodos((s) => s.snap);
  const cols = snap?.settings.columns ?? [];
  const col = cols.find((c) => c.id === id);
  const rest = cols.filter((c) => c.id !== id);
  const n = columnCounts(snap?.tasks ?? [], cols).get(id) ?? 0;
  const [to, setTo] = useState(() => firstColumn(rest)?.id ?? '');
  const error = columnsError(rest);
  if (!col) return null;
  const save = async () => {
    const r = await saveColumns(rest, n && to ? { [id]: to } : undefined, `Deleted column ${col.name}`);
    if (r) close();
  };
  return (
    <Modal
      open
      onClose={close}
      title={`Delete “${col.name}”?`}
      footer={
        <>
          <button className="btn" onClick={close}>
            Cancel
          </button>
          <button className="btn btn-danger" onClick={() => void save()} disabled={!!error}>
            Delete column
          </button>
        </>
      }
    >
      <div className="col">
        {n > 0 ? (
          <Field label={`Its ${n} open task${n === 1 ? '' : 's'} move to`}>
            <select className="select" value={to} onChange={(e) => setTo(e.target.value)} autoFocus>
              {rest.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                  {c.done ? ' — completes them' : ''}
                </option>
              ))}
            </select>
          </Field>
        ) : (
          <p className="small muted">The column is empty. Completed tasks that came from it reopen in the first column.</p>
        )}
        {error && <div className="td-col-error small">{error}</div>}
      </div>
    </Modal>
  );
}

const SHORTCUTS: [string, string][] = [
  ['Q', 'Quick add'],
  ['/', 'Search'],
  ['F', 'Filter this view (Esc clears)'],
  ['J / K or ↓ / ↑', 'Move the selection'],
  ['Enter or E', 'Edit the task'],
  ['O or Shift+Enter', 'Open details'],
  [`Space or ${MOD}+Enter`, 'Complete'],
  ['1 – 4', 'Priority'],
  ['T / M / N', 'Due today / tomorrow / no date'],
  [`${MOD}+] / ${MOD}+[`, 'Indent / outdent (sub-task)'],
  ['Del', 'Delete'],
  [`${MOD}+A`, 'Select all'],
  [`${MOD}/Shift + click`, 'Multi-select'],
  ['Esc', 'Clear selection / close'],
];

const BOARD_SHORTCUTS: [string, string][] = [
  ['← → ↑ ↓ (J / K)', 'Move between cards'],
  ['Enter', 'Open the card'],
  ['N', 'New card in the column'],
  ['E', 'Quick edit'],
  ['L / D', 'Labels / due date'],
  ['C', 'Complete'],
  ['Space', 'Select (drag selected cards together)'],
  ['< / >', 'Move to the previous / next column'],
];

const SYNTAX: [string, string][] = [
  ['tomorrow 5pm, next fri, in 3 days, jan 15, 15th', 'due date'],
  ['every day, every weekday, every mon, wed', 'repeats from the due date'],
  ['every 2 weeks, every month on the 1st, every last day', ''],
  ['every! 3 days', 'repeats from when you complete it'],
  ['#List  /Section', 'where it goes'],
  ['@label', 'labels (new ones are created)'],
  ['p1 … p4', 'priority (p1 = most urgent)'],
  ['{fri}', 'deadline'],
];

function HelpDialog() {
  return (
    <Modal open onClose={close} title="To-do shortcuts" size="lg">
      <div className="col-lg">
        <div className="td-kbd-list">
          {SHORTCUTS.map(([k, d]) => (
            <FragmentKbd key={k} k={k} d={d} />
          ))}
        </div>
        <div>
          <div className="td-section-title">On a board</div>
          <div className="td-kbd-list">
            {BOARD_SHORTCUTS.map(([k, d]) => (
              <FragmentKbd key={k} k={k} d={d} />
            ))}
          </div>
        </div>
        <div>
          <div className="td-section-title">Quick add syntax</div>
          <div className="td-syntax">
            {SYNTAX.map(([q, d]) => (
              <FragmentSyntax key={q} q={q} d={d} />
            ))}
          </div>
          <p className="subtle xs" style={{ marginTop: 'var(--space-4)' }}>
            Recognised parts are highlighted as you type; click × on a chip to keep that word as text.
          </p>
        </div>
      </div>
    </Modal>
  );
}

function FragmentKbd({ k, d }: { k: string; d: string }) {
  return (
    <>
      <span>
        <Kbd>{k}</Kbd>
      </span>
      <span>{d}</span>
    </>
  );
}

function FragmentSyntax({ q, d }: { q: string; d: string }) {
  return (
    <>
      <code>{q}</code>
      <span className="subtle">{d}</span>
    </>
  );
}

/** Renders the open to-do dialog (mounted by the page). */
export function TodoDialogs() {
  const spec = useDialog((s) => s.spec);
  if (!spec) return null;
  switch (spec.kind) {
    case 'list':
      return <ListDialog key={spec.id ?? 'new'} id={spec.id} />;
    case 'label':
      return <LabelDialog key={spec.name ?? 'new'} name={spec.name} />;
    case 'mergeLabel':
      return <MergeLabelDialog key={spec.name} name={spec.name} />;
    case 'filter':
      return <FilterDialog key={spec.id ?? 'new'} id={spec.id} query={spec.query} />;
    case 'settings':
      return <SettingsDialog />;
    case 'help':
      return <HelpDialog />;
    case 'columns':
      return <ColumnsDialog />;
    case 'deleteColumn':
      return <DeleteColumnDialog key={spec.id} id={spec.id} />;
  }
}
