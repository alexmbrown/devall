import { useEffect, useState } from 'react';
import { create } from 'zustand';
import { BellRing } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Field, Kbd, MOD, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { todosApi, type TodoSettings } from '@/lib/api/todos';
import { useTodos } from '@/store/todos';
import { ColorPicker } from './pickers';

type DialogSpec = { kind: 'list'; id?: string } | { kind: 'label'; name?: string } | { kind: 'filter'; id?: string; query?: string } | { kind: 'settings' } | { kind: 'help' };

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

function LabelDialog({ name: initial }: { name?: string }) {
  const label = useTodos((s) => s.snap?.labels.find((l) => l.name === initial));
  const [name, setName] = useState(initial ?? '');
  const [color, setColor] = useState<string | null>(label?.color ?? null);
  const clean = name.trim().replace(/^@/, '');
  const valid = clean && !/\s/.test(clean);
  const save = async () => {
    if (!valid) return;
    const r = await useTodos.getState().apply(initial ? { type: 'saveLabel', name: initial, newName: clean, color } : { type: 'saveLabel', name: clean, color });
    if (r) {
      close();
      const v = useTodos.getState().view;
      if (initial && v?.kind === 'label' && v.name === initial) useTodos.getState().setView({ kind: 'label', name: clean });
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
            {initial ? 'Save' : 'Create label'}
          </button>
        </>
      }
    >
      <div className="col">
        <Field label="Name" error={clean && !valid ? 'One word (use - or _)' : undefined} hint="Type @name in quick add to label a task.">
          <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && void save()} placeholder="e.g. waiting-on" />
        </Field>
        <Field label="Colour">
          <ColorPicker value={color} onChange={setColor} />
        </Field>
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
  const [s, setS] = useState<TodoSettings>(current ?? { reminders: true, remindBeforeMinutes: 0 });
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

const SHORTCUTS: [string, string][] = [
  ['Q', 'Quick add'],
  ['/', 'Search'],
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
    case 'filter':
      return <FilterDialog key={spec.id ?? 'new'} id={spec.id} query={spec.query} />;
    case 'settings':
      return <SettingsDialog />;
    case 'help':
      return <HelpDialog />;
  }
}
