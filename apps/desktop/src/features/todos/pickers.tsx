import { useEffect, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Check, CheckCircle2, Flag, Hash, Inbox, Palette, Plus, Rows3, X } from 'lucide-react';
import type { Priority, StatusColumn } from '@/lib/api/todos';
import type { MenuItemDef } from '@/components/ui/Menu';
import { useTodos } from '@/store/todos';
import { Popover } from './Popover';
import { COLORS, colorVar, isHexColor, labelColor } from './lib';

/** A label as a coloured chip (readable in both themes: tinted background, text mixed toward the text colour). */
export function LabelChip({
  name,
  color,
  onRemove,
  onColor,
  onClick,
  className,
  title,
}: {
  name: string;
  /** Palette name or `#hex` (see `labelColor`). */
  color: string;
  onRemove?(): void;
  /** Makes the colour dot a button (pick the label's colour). */
  onColor?(e: React.MouseEvent): void;
  onClick?(e: React.MouseEvent): void;
  className?: string;
  title?: string;
}) {
  return (
    <span className={clsx('td-lchip', onClick && 'clickable', className)} style={{ ['--c' as string]: colorVar(color) }} title={title ?? `@${name}`} onClick={onClick}>
      {onColor ? (
        <button
          className="td-lchip-dot"
          onClick={(e) => {
            e.stopPropagation();
            onColor(e);
          }}
          title={`Colour of @${name}`}
          aria-label={`Colour of ${name}`}
        />
      ) : (
        <span className="td-lchip-dot" aria-hidden />
      )}
      <span className="truncate">{name}</span>
      {onRemove && (
        <button
          className="td-lchip-x"
          onClick={(e) => {
            e.stopPropagation();
            onRemove();
          }}
          aria-label={`Remove ${name}`}
          title={`Remove @${name}`}
        >
          <X />
        </button>
      )}
    </span>
  );
}

/** Saves a label's colour (`null` = its name's default colour), keeping its name. */
export function setLabelColor(name: string, color: string | null) {
  void useTodos.getState().apply({ type: 'saveLabel', name, color });
}

/** The colour picker of one label, anchored to a chip, a nav item or a picker row. */
export function LabelColorPopover({ name, anchor, onClose }: { name: string; anchor: DOMRect; onClose(): void }) {
  const label = useTodos((s) => s.snap?.labels.find((l) => l.name === name));
  return (
    <Popover anchor={anchor} onClose={onClose}>
      <div className="td-pop-title">
        Colour of <strong className="truncate">@{name}</strong>
      </div>
      <ColorPicker value={label?.color ?? null} auto={labelColor(name, [])} custom onChange={(c) => setLabelColor(name, c)} noneLabel="Default colour (from the name)" />
    </Popover>
  );
}

/** Toggles labels (creating new ones). `selected` = labels every target task has. The dot of a row sets its colour. */
export function LabelPicker({ anchor, onClose, selected, onToggle }: { anchor: DOMRect; onClose(): void; selected: string[]; onToggle(name: string, on: boolean): void }) {
  const labels = useTodos((s) => s.snap?.labels ?? []);
  const [q, setQ] = useState('');
  const [colouring, setColouring] = useState<string | null>(null);
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
            <div key={l.name}>
              <div className="td-pop-row">
                <button
                  className="td-swatch-btn"
                  style={{ ['--c' as string]: colorVar(labelColor(l.name, labels)) }}
                  onClick={() => setColouring(colouring === l.name ? null : l.name)}
                  title={`Colour of @${l.name}`}
                  aria-label={`Colour of ${l.name}`}
                  aria-expanded={colouring === l.name}
                />
                <button className="td-pop-item" onClick={() => onToggle(l.name, !on)}>
                  <span className="grow truncate">{l.name}</span>
                  {on && <Check />}
                </button>
              </div>
              {colouring === l.name && (
                <div className="td-pop-colors">
                  <ColorPicker value={l.color ?? null} auto={labelColor(l.name, [])} custom onChange={(c) => setLabelColor(l.name, c)} noneLabel="Default colour (from the name)" />
                </div>
              )}
            </div>
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
            <span className="td-swatch" style={{ background: colorVar(labelColor(name, [])) }} />
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

/** `#abc` → `#aabbcc` (what `<input type="color">` takes). */
const longHex = (c: string) => (c.length === 4 ? `#${[...c.slice(1)].map((x) => x + x).join('')}` : c).toLowerCase();

/**
 * The palette (and, with `custom`, any colour: the system colour picker or a typed `#hex`).
 * `auto`: the colour "none" stands for (labels: the name's default colour), shown in its swatch.
 */
export function ColorPicker({ value, onChange, custom, auto, noneLabel = 'No colour' }: { value?: string | null; onChange(c: string | null): void; custom?: boolean; auto?: string; noneLabel?: string }) {
  const hexValue = isHexColor(value) ? longHex(value) : '';
  const [hex, setHex] = useState(hexValue);
  useEffect(() => setHex(hexValue), [hexValue]);
  // The system picker reports every move: save once it settles.
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const typed = hex.trim().startsWith('#') ? hex.trim() : `#${hex.trim()}`;
  const commit = () => {
    if (isHexColor(typed) && longHex(typed) !== hexValue) onChange(longHex(typed));
  };
  return (
    <div className="td-colors-wrap">
      <div className="td-colors" role="radiogroup" aria-label="Colour">
        <button className={clsx('none', auto && 'auto', !value && 'on')} style={auto ? { ['--c' as string]: colorVar(auto) } : undefined} onClick={() => onChange(null)} title={noneLabel} aria-label={noneLabel} />
        {COLORS.map((c) => (
          <button key={c} className={clsx(value === c && 'on')} style={{ ['--c' as string]: colorVar(c) }} onClick={() => onChange(c)} title={c} aria-label={c} />
        ))}
        {custom && (
          <label className={clsx('custom', hexValue && 'on')} style={hexValue ? { ['--c' as string]: hexValue } : undefined} title="Custom colour…">
            <Palette aria-hidden />
            <input
              type="color"
              value={hexValue || '#888888'}
              aria-label="Custom colour"
              onChange={(e) => {
                const c = e.target.value;
                setHex(c);
                clearTimeout(timer.current);
                timer.current = setTimeout(() => onChange(c), 250);
              }}
            />
          </label>
        )}
      </div>
      {custom && (
        <input
          className={clsx('input sm mono td-hex', hex.trim() && !isHexColor(typed) && 'invalid')}
          placeholder="#rrggbb"
          aria-label="Hex colour"
          value={hex}
          maxLength={7}
          onChange={(e) => setHex(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && commit()}
        />
      )}
    </div>
  );
}
