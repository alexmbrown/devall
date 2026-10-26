import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { AtSign, CalendarClock, Flag, Hash, Repeat, Rows3, Target, X } from 'lucide-react';
import { todosApi, type QuickAdd, type QuickToken } from '@/lib/api/todos';
import { useTodos } from '@/store/todos';
import { colorVar, dayLabel, labelColor, segments, sigilAt, suggest, timeLabel, todayYmd } from './lib';

export interface QuickAddInputHandle {
  focus(): void;
  /** The latest parse (may lag one keystroke behind). */
  parsed(): QuickAdd | null;
}

interface Props {
  value: string;
  onChange(v: string): void;
  /** Enter (when no suggestion is open). */
  onSubmit(): void;
  onCancel?(): void;
  onBlur?(): void;
  /** List used to resolve `/section` (default: the Inbox). */
  listId?: string | null;
  /** Token texts the user turned off (chip ×). */
  ignore: string[];
  onIgnore(next: string[]): void;
  placeholder?: string;
  autoFocus?: boolean;
  big?: boolean;
  /** Show the parsed chips under the input. */
  chips?: boolean;
  className?: string;
}

function chip(t: QuickToken, q: QuickAdd, lists: Map<string, string>, sections: Map<string, string>, today: string): { icon: React.ReactNode; text: string } {
  switch (t.kind) {
    case 'date': {
      if (!q.due) return { icon: <CalendarClock />, text: 'No date' };
      const time = q.due.time ? ` ${timeLabel(q.due.time)}` : '';
      return { icon: q.due.recurrence ? <Repeat /> : <CalendarClock />, text: q.due.recurrence ? `${q.due.string || 'Repeats'} · next ${dayLabel(q.due.date, today)}${time}` : `${dayLabel(q.due.date, today)}${time}` };
    }
    case 'deadline':
      return { icon: <Target />, text: `Deadline ${dayLabel(t.value, today)}` };
    case 'list':
      return { icon: <Hash />, text: lists.get(t.value) ?? t.text.slice(1) };
    case 'section':
      return { icon: <Rows3 />, text: sections.get(t.value) ?? t.text.slice(1) };
    case 'label':
      return { icon: <AtSign />, text: `${t.value}${t.new ? ' (new)' : ''}` };
    case 'priority':
      return { icon: <Flag />, text: t.value.toUpperCase() };
  }
}

/**
 * Quick-add text field: Todoist syntax (`tomorrow 5pm #Work /Backend @api p1 {fri}`) is parsed live by
 * the Rust parser and highlighted in place; chips below can turn a token off. Typing `#`, `@` or `/`
 * suggests lists, labels and sections (Tab/Enter to complete).
 */
export const QuickAddInput = forwardRef<QuickAddInputHandle, Props>(function QuickAddInput({ value, onChange, onSubmit, onCancel, onBlur, listId, ignore, onIgnore, placeholder, autoFocus, big, chips = true, className }, ref) {
  const root = useTodos((s) => s.root);
  const snap = useTodos((s) => s.snap);
  const [parsed, setParsed] = useState<QuickAdd | null>(null);
  const [caret, setCaret] = useState(0);
  const [hl, setHl] = useState(0);
  const [sugOff, setSugOff] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const mirror = useRef<HTMLDivElement>(null);
  const req = useRef(0);
  const latest = useRef<QuickAdd | null>(null);

  useImperativeHandle(ref, () => ({ focus: () => input.current?.focus(), parsed: () => latest.current }), []);

  const ignoreKey = ignore.join('\u0000');
  useEffect(() => {
    if (!value.trim()) {
      setParsed(null);
      latest.current = null;
      return;
    }
    const my = ++req.current;
    const t = setTimeout(() => {
      todosApi
        .parse(root, value, listId, ignore)
        .then((q) => {
          if (my !== req.current) return;
          latest.current = q;
          setParsed(q);
        })
        .catch(() => {});
    }, 50);
    return () => clearTimeout(t);
  }, [value, listId, ignoreKey, root]); // `ignore` is tracked by its joined key

  const segs = useMemo(() => segments(value, parsed?.tokens ?? []), [value, parsed]);
  const sync = () => {
    if (mirror.current && input.current) mirror.current.scrollLeft = input.current.scrollLeft;
    setCaret(input.current?.selectionStart ?? value.length);
  };
  useLayoutEffect(sync);

  const lists = useMemo(() => new Map((snap?.lists ?? []).map((l) => [l.id, l.name])), [snap]);
  const sections = useMemo(() => new Map((snap?.sections ?? []).map((s) => [s.id, s.name])), [snap]);
  const at = sugOff ? null : sigilAt(value, caret);
  const sectionList = parsed?.listId ?? listId ?? 'inbox';
  const options = useMemo(() => {
    if (!at || !snap) return [];
    const names = at.sigil === '#' ? snap.lists.map((l) => l.name) : at.sigil === '@' ? snap.labels.map((l) => l.name) : snap.sections.filter((s) => s.listId === sectionList).map((s) => s.name);
    return suggest(names, at.partial);
  }, [at, snap, sectionList]);
  useEffect(() => setHl(0), [at?.partial, at?.sigil]);

  const complete = (name: string) => {
    if (!at) return;
    const next = `${value.slice(0, at.start)}${at.sigil}${name} ${value.slice(caret).replace(/^\s+/, '')}`;
    onChange(next);
    const pos = at.start + name.length + 2;
    requestAnimationFrame(() => {
      input.current?.setSelectionRange(pos, pos);
      sync();
    });
  };

  const today = snap?.today ?? todayYmd();
  return (
    <div className={clsx('td-qa', big && 'big', className)}>
      <div className="td-qa-field">
        <div ref={mirror} className="input td-qa-mirror" aria-hidden>
          <span>
            {segs.map((s, i) =>
              s.token ? (
                <mark key={i} className={clsx('td-tok', s.token.kind)} style={s.token.kind === 'label' ? { ['--c' as string]: colorVar(labelColor(s.token.value, snap?.labels)) } : undefined}>
                  {s.text}
                </mark>
              ) : (
                <span key={i}>{s.text}</span>
              ),
            )}
            {'​'}
          </span>
        </div>
        <input
          ref={input}
          className="input td-qa-input"
          value={value}
          placeholder={placeholder}
          autoFocus={autoFocus}
          spellCheck={false}
          onChange={(e) => {
            setSugOff(false);
            onChange(e.target.value);
          }}
          onScroll={sync}
          onSelect={sync}
          onKeyUp={sync}
          onBlur={() => {
            setSugOff(true);
            onBlur?.();
          }}
          onFocus={() => setSugOff(false)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (options.length) {
              if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                e.preventDefault();
                setHl((h) => (h + (e.key === 'ArrowDown' ? 1 : options.length - 1)) % options.length);
                return;
              }
              if (e.key === 'Tab' || (e.key === 'Enter' && !e.ctrlKey && !e.metaKey)) {
                e.preventDefault();
                complete(options[hl] ?? options[0]);
                return;
              }
              if (e.key === 'Escape') {
                e.preventDefault();
                setSugOff(true);
                return;
              }
            }
            if (e.key === 'Enter') {
              e.preventDefault();
              onSubmit();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              onCancel?.();
            }
          }}
        />
        {options.length > 0 && (
          <div className="menu" style={{ position: 'absolute', top: '100%', left: 0, marginTop: 4 }} onMouseDown={(e) => e.preventDefault()}>
            {options.map((o, i) => (
              <button key={o} className={clsx('menu-item', i === hl && 'hl')} onMouseEnter={() => setHl(i)} onClick={() => complete(o)}>
                {at?.sigil === '#' ? <Hash /> : at?.sigil === '@' ? <AtSign /> : <Rows3 />}
                <span className="truncate">{o}</span>
              </button>
            ))}
          </div>
        )}
      </div>
      {chips && parsed && parsed.tokens.length > 0 && (
        <div className="td-chips">
          {parsed.tokens
            .filter((t) => value.slice(t.start, t.end) === t.text)
            .map((t) => {
              const c = chip(t, parsed, lists, sections, today);
              return (
                <span
                  key={`${t.kind}${t.start}`}
                  className={clsx('td-chip', t.kind, t.new && 'new')}
                  style={t.kind === 'label' ? { ['--c' as string]: colorVar(labelColor(t.value, snap?.labels)) } : undefined}
                  title={`“${t.text}”`}
                >
                  {c.icon}
                  {c.text}
                  <button onMouseDown={(e) => e.preventDefault()} onClick={() => onIgnore([...ignore, t.text])} title={`Keep “${t.text}” as text`} aria-label="Keep as text">
                    <X />
                  </button>
                </span>
              );
            })}
        </div>
      )}
    </div>
  );
});
