import { useMemo, useState } from 'react';
import clsx from 'clsx';
import { AtSign, Check, CheckCircle2, Flag, Hash, Inbox, Plus, Rows3 } from 'lucide-react';
import type { Priority, StatusColumn } from '@/lib/api/todos';
import type { MenuItemDef } from '@/components/ui/Menu';
import { useTodos } from '@/store/todos';
import { Popover } from './Popover';
import { COLORS, colorVar } from './lib';

/** Toggles labels (creating new ones). `selected` = labels every target task has. */
export function LabelPicker({ anchor, onClose, selected, onToggle }: { anchor: DOMRect; onClose(): void; selected: string[]; onToggle(name: string, on: boolean): void }) {
  const labels = useTodos((s) => s.snap?.labels ?? []);
  const [q, setQ] = useState('');
  const name = q.trim().replace(/^@/, '').replace(/\s+/g, '-');
  const shown = labels.filter((l) => l.name.toLowerCase().includes(name.toLowerCase()));
  const exists = labels.some((l) => l.name.toLowerCase() === name.toLowerCase());
  return (
    <Popover anchor={anchor} onClose={onClose}>
      <input
        className="input sm"
        autoFocus
        placeholder="Label name"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && name) {
            const hit = shown[0]?.name;
            if (!exists && !hit) onToggle(name, true);
            else if (hit) onToggle(hit, !selected.includes(hit));
            setQ('');
          }
        }}
      />
      <div className="td-pop-section">
        {shown.map((l) => {
          const on = selected.includes(l.name);
          return (
            <button key={l.name} className="td-pop-item" onClick={() => onToggle(l.name, !on)}>
              <AtSign style={{ color: colorVar(l.color) }} />
              <span className="grow truncate">{l.name}</span>
              {on && <Check />}
            </button>
          );
        })}
        {name && !exists && (
          <button
            className="td-pop-item"
            onClick={() => {
              onToggle(name, true);
              setQ('');
            }}
          >
            <Plus />
            Create “{name}”
          </button>
        )}
        {!labels.length && !name && <div className="subtle xs" style={{ padding: 'var(--space-3)' }}>No labels yet — type one, or add @label in quick add.</div>}
      </div>
    </Popover>
  );
}

/** Picks a list (and section). */
export function MovePicker({ anchor, onClose, onPick, current }: { anchor: DOMRect; onClose(): void; onPick(listId: string, sectionId: string | null): void; current?: { listId: string; sectionId?: string | null } }) {
  const snap = useTodos((s) => s.snap);
  const [q, setQ] = useState('');
  const items = useMemo(() => {
    const out: { listId: string; sectionId: string | null; label: string; icon: React.ReactNode; indent?: boolean; color?: string }[] = [];
    for (const l of snap?.lists ?? []) {
      out.push({ listId: l.id, sectionId: null, label: l.name, icon: l.kind === 'inbox' ? <Inbox /> : <Hash />, color: colorVar(l.color) });
      for (const s of (snap?.sections ?? []).filter((s) => s.listId === l.id)) out.push({ listId: l.id, sectionId: s.id, label: s.name, icon: <Rows3 />, indent: true });
    }
    const needle = q.trim().toLowerCase();
    return needle ? out.filter((i) => i.label.toLowerCase().includes(needle)) : out;
  }, [snap, q]);
  return (
    <Popover anchor={anchor} onClose={onClose}>
      <input className="input sm" autoFocus placeholder="Move to…" value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={(e) => e.key === 'Enter' && items[0] && (onPick(items[0].listId, items[0].sectionId), onClose())} />
      <div className="td-pop-section">
        {items.map((i) => {
          const on = current && current.listId === i.listId && (current.sectionId ?? null) === i.sectionId;
          return (
            <button
              key={`${i.listId}/${i.sectionId}`}
              className={clsx('td-pop-item', i.indent && !q && 'indent')}
              onClick={() => {
                onPick(i.listId, i.sectionId);
                onClose();
              }}
            >
              <span style={{ color: i.color, display: 'inline-flex' }}>{i.icon}</span>
              <span className="grow truncate">{i.label}</span>
              {on && <Check />}
            </button>
          );
        })}
      </div>
    </Popover>
  );
}

export function PriorityIcon({ p }: { p: Priority }) {
  return <Flag style={{ color: `var(--td-p${p})` }} fill={p < 4 ? `var(--td-p${p})` : 'none'} />;
}

export function priorityItems(current: Priority | undefined, onPick: (p: Priority) => void): MenuItemDef[] {
  return ([1, 2, 3, 4] as Priority[]).map((p) => ({ label: `Priority ${p}`, icon: <PriorityIcon p={p} />, hint: current === p ? '✓' : String(p), onSelect: () => onPick(p) }));
}

/** A status column's marker: its colour dot, a check for the done column. */
export function StatusDot({ col }: { col?: StatusColumn }) {
  return col?.done ? <CheckCircle2 className="td-status-done" style={{ color: colorVar(col.color) }} /> : <span className="td-swatch" style={{ background: colorVar(col?.color) }} />;
}

/** Menu items to pick a status column (`current`: the task's column id). */
export function statusItems(cols: StatusColumn[], current: string | undefined, onPick: (c: StatusColumn) => void): MenuItemDef[] {
  return cols.map((c) => ({ label: c.name, icon: <StatusDot col={c} />, hint: current === c.id ? '✓' : undefined, onSelect: () => onPick(c) }));
}

export function ColorPicker({ value, onChange }: { value?: string | null; onChange(c: string | null): void }) {
  return (
    <div className="td-colors" role="radiogroup" aria-label="Colour">
      <button className={clsx('none', !value && 'on')} onClick={() => onChange(null)} title="No colour" aria-label="No colour" />
      {COLORS.map((c) => (
        <button key={c} className={clsx(value === c && 'on')} style={{ ['--c' as string]: colorVar(c) }} onClick={() => onChange(c)} title={c} aria-label={c} />
      ))}
    </div>
  );
}
