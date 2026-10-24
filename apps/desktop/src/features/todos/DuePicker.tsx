import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { Ban, CalendarCheck, CalendarDays, ChevronLeft, ChevronRight, Repeat, Sofa, Sun, Sunrise, X } from 'lucide-react';
import { todosApi, type Due, type TaskPatch } from '@/lib/api/todos';
import { Popover } from './Popover';
import { addDays, dayLabel, monthGrid, parseYmd, ruleText, startOfWeek, timeLabel } from './lib';

function Calendar({ selected, today, onPick }: { selected?: string; today: string; onPick(d: string): void }) {
  const [month, setMonth] = useState(() => (selected ?? today).slice(0, 7));
  const days = monthGrid(`${month}-01`);
  const shift = (n: number) => {
    const d = parseYmd(`${month}-01`);
    d.setMonth(d.getMonth() + n);
    setMonth(`${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}`);
  };
  return (
    <div>
      <div className="td-cal-head">
        <span className="grow">{parseYmd(`${month}-01`).toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}</span>
        <button className="icon-btn sm" onClick={() => shift(-1)} aria-label="Previous month">
          <ChevronLeft />
        </button>
        <button className="icon-btn sm" onClick={() => setMonth(today.slice(0, 7))} title="This month" aria-label="This month">
          <CalendarDays />
        </button>
        <button className="icon-btn sm" onClick={() => shift(1)} aria-label="Next month">
          <ChevronRight />
        </button>
      </div>
      <div className="td-cal">
        {['M', 'T', 'W', 'T', 'F', 'S', 'S'].map((d, i) => (
          <span key={i} className="wd">
            {d}
          </span>
        ))}
        {days.map((d) => (
          <button key={d} className={clsx(d.slice(0, 7) !== month && 'other', d < today && 'past', d === today && 'today', d === selected && 'sel')} onClick={() => onPick(d)} title={parseYmd(d).toDateString()}>
            {Number(d.slice(8))}
          </button>
        ))}
      </div>
    </div>
  );
}

/** Natural-language input with a live preview (parsed by the Rust date parser). */
function useNlPreview(text: string, dateOnly: boolean) {
  const [state, setState] = useState<{ ok: boolean; due: Due | null; error?: string } | null>(null);
  const req = useRef(0);
  useEffect(() => {
    if (!text.trim()) {
      setState(null);
      return;
    }
    const my = ++req.current;
    const t = setTimeout(() => {
      todosApi
        .parseDate(text, dateOnly)
        .then((due) => my === req.current && setState({ ok: true, due }))
        .catch((e) => my === req.current && setState({ ok: false, due: null, error: String(e) }));
    }, 80);
    return () => clearTimeout(t);
  }, [text, dateOnly]);
  return state;
}

function previewText(due: Due | null, today: string) {
  if (!due) return 'No date';
  const time = due.time ? ` ${timeLabel(due.time)}` : '';
  const rule = due.recurrence ? ` · ${ruleText(due)}` : '';
  return `${dayLabel(due.date, today)} (${parseYmd(due.date).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' })})${time}${rule}`;
}

/** Picks a due date (`mode: 'due'`) or a deadline; reports a `TaskPatch`. */
export function DuePicker({ anchor, onClose, mode, due, deadline, today, onPatch }: { anchor: DOMRect; onClose(): void; mode: 'due' | 'deadline'; due?: Due | null; deadline?: string | null; today: string; onPatch(p: TaskPatch): void }) {
  const [text, setText] = useState('');
  const preview = useNlPreview(text, mode === 'deadline');
  const pick = (p: TaskPatch) => {
    onPatch(p);
    onClose();
  };
  const sat = (() => {
    const d = parseYmd(today).getDay();
    return d === 6 || d === 0 ? today : addDays(startOfWeek(today), 5);
  })();
  const nextMon = addDays(startOfWeek(today), 7);
  const quick: { icon: React.ReactNode; label: string; date: string }[] = [
    { icon: <CalendarCheck />, label: 'Today', date: today },
    { icon: <Sunrise />, label: 'Tomorrow', date: addDays(today, 1) },
    { icon: <Sofa />, label: 'This weekend', date: sat },
    { icon: <Sun />, label: 'Next week', date: nextMon },
  ];
  const set = (date: string) => (mode === 'due' ? { dueDate: date } : { deadline: date });
  const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const current = mode === 'due' ? due?.date : (deadline ?? undefined);

  return (
    <Popover anchor={anchor} onClose={onClose} className="wide">
      <input
        className="input"
        autoFocus
        value={text}
        placeholder={mode === 'due' ? 'Type a date: next fri 5pm, every mon…' : 'Type a deadline: fri, jan 30…'}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && preview?.ok) pick(mode === 'due' ? { dueString: text } : { deadline: text });
        }}
      />
      <div className={clsx('td-preview', preview?.ok && 'ok', preview && !preview.ok && 'err')}>{preview ? (preview.ok ? `${previewText(preview.due, today)} — Enter to set` : preview.error) : mode === 'due' ? (due ? previewText(due, today) : 'No date') : deadline ? `Deadline ${dayLabel(deadline, today)}` : 'No deadline'}</div>
      <div className="td-pop-section">
        {quick.map((q) => (
          <button key={q.label} className="td-pop-item" onClick={() => pick(set(q.date))}>
            {q.icon}
            {q.label}
            <span className="hint">{parseYmd(q.date).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' })}</span>
          </button>
        ))}
        {(mode === 'due' ? due : deadline) && (
          <button className="td-pop-item" onClick={() => pick(mode === 'due' ? { dueDate: '' } : { deadline: null })}>
            <Ban />
            {mode === 'due' ? 'No date' : 'No deadline'}
          </button>
        )}
      </div>
      <Calendar selected={current} today={today} onPick={(d) => pick(set(d))} />
      {mode === 'due' && due && (
        <div className="col-sm" style={{ marginTop: 'var(--space-4)', borderTop: '1px solid var(--border)', paddingTop: 'var(--space-4)' }}>
          <div className="row-sm">
            <span className="subtle xs" style={{ width: 54 }}>
              Time
            </span>
            <input
              type="time"
              className="input sm"
              style={{ width: 120 }}
              value={due.time ?? ''}
              onChange={(e) => onPatch({ due: { ...due, time: e.target.value || undefined, timezone: e.target.value ? due.timezone : undefined } })}
            />
            {due.time && (
              <button className="icon-btn sm" onClick={() => onPatch({ due: { ...due, time: undefined, timezone: undefined } })} title="Remove the time" aria-label="Remove the time">
                <X />
              </button>
            )}
          </div>
          {due.time && (
            <label className="check xs" title="Keep this time in a fixed time zone when you travel (otherwise it follows your clock)">
              <input type="checkbox" checked={!!due.timezone} onChange={(e) => onPatch({ due: { ...due, timezone: e.target.checked ? tz : undefined } })} />
              <span>Fixed time zone{due.timezone ? ` (${due.timezone})` : ` (${tz})`}</span>
            </label>
          )}
          {due.recurrence && (
            <div className="row-sm xs">
              <Repeat size={13} className="subtle" />
              <span className="grow truncate">{ruleText(due)}</span>
              <button className="btn btn-sm btn-ghost" onClick={() => pick({ due: { ...due, recurrence: undefined, string: undefined } })}>
                Stop repeating
              </button>
            </div>
          )}
        </div>
      )}
    </Popover>
  );
}
