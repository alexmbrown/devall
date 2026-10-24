import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';
import { openReference, segments } from '@/lib/template';
import type { VarInfo } from '@/lib/types';

interface Props {
  value: string;
  onChange(v: string): void;
  vars: VarInfo[];
  placeholder?: string;
  className?: string;
  onEnter?(): void;
  autoFocus?: boolean;
  mono?: boolean;
  title?: string;
}

/**
 * Single-line input that colours `{{references}}` (known = accent, unknown = danger), shows the
 * resolved value on hover and autocompletes after `{{`.
 */
export function VarInput({ value, onChange, vars, placeholder, className, onEnter, autoFocus, mono = true, title }: Props) {
  const inputRef = useRef<HTMLInputElement>(null);
  const mirrorRef = useRef<HTMLDivElement>(null);
  const [caret, setCaret] = useState<number | null>(null);
  const [hl, setHl] = useState(0);
  const byName = useMemo(() => new Map(vars.map((v) => [v.name, v])), [vars]);
  const segs = useMemo(() => segments(value), [value]);

  const open = caret !== null ? openReference(value, caret) : null;
  const suggestions = useMemo(() => {
    if (!open) return [];
    const q = open.query.toLowerCase();
    return vars.filter((v) => v.name.toLowerCase().includes(q)).slice(0, 12);
  }, [open, vars]);

  useLayoutEffect(() => {
    if (mirrorRef.current && inputRef.current) mirrorRef.current.scrollLeft = inputRef.current.scrollLeft;
  });

  const insert = (name: string) => {
    if (!open || caret === null) return;
    const next = `${value.slice(0, open.start)}{{${name}}}${value.slice(caret).replace(/^[^\s{}]*}}/, '')}`;
    onChange(next);
    const pos = open.start + name.length + 4;
    setCaret(null);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(pos, pos);
    });
  };

  const rect = inputRef.current?.getBoundingClientRect();
  const hover = segs
    .filter((s) => s.kind === 'var')
    .map((s) => (s.kind === 'var' ? `${s.name} = ${byName.get(s.name)?.value ?? '(not set)'}` : ''))
    .join('\n');

  return (
    <div className={clsx('var-input', mono && 'mono', className)} title={title ?? (hover || undefined)}>
      <div ref={mirrorRef} className="var-mirror" aria-hidden>
        {segs.map((s, i) =>
          s.kind === 'text' ? (
            <span key={i}>{s.text}</span>
          ) : (
            <span key={i} className={clsx('var-ref', byName.has(s.name) || s.name.startsWith('$env.') ? 'known' : 'unknown')}>
              {s.text}
            </span>
          ),
        )}
        {!value && <span className="var-placeholder">{placeholder}</span>}
      </div>
      <input
        ref={inputRef}
        value={value}
        spellCheck={false}
        autoFocus={autoFocus}
        onChange={(e) => {
          onChange(e.target.value);
          setCaret(e.target.selectionStart);
          setHl(0);
        }}
        onSelect={(e) => setCaret(e.currentTarget.selectionStart)}
        onScroll={(e) => mirrorRef.current && (mirrorRef.current.scrollLeft = e.currentTarget.scrollLeft)}
        onBlur={() => setTimeout(() => setCaret(null), 120)}
        onKeyDown={(e) => {
          if (suggestions.length) {
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              return setHl((h) => (h + 1) % suggestions.length);
            }
            if (e.key === 'ArrowUp') {
              e.preventDefault();
              return setHl((h) => (h - 1 + suggestions.length) % suggestions.length);
            }
            if (e.key === 'Enter' || e.key === 'Tab') {
              e.preventDefault();
              return insert(suggestions[hl].name);
            }
            if (e.key === 'Escape') return setCaret(null);
          }
          if (e.key === 'Enter' && !e.shiftKey) onEnter?.();
        }}
      />
      {suggestions.length > 0 &&
        rect &&
        createPortal(
          <div className="menu var-suggest" style={{ left: rect.left, top: rect.bottom + 4, minWidth: Math.min(420, rect.width) }}>
            {suggestions.map((s, i) => (
              <button
                key={s.name}
                className={clsx('menu-item', i === hl && 'hl')}
                onMouseDown={(e) => {
                  e.preventDefault();
                  insert(s.name);
                }}
                onMouseEnter={() => setHl(i)}
              >
                <span className="mono strong">{s.name}</span>
                <span className="truncate subtle mono xs">{s.value ?? ''}</span>
                <span className="hint">{s.source}</span>
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
