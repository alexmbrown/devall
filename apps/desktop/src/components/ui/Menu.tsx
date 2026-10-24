import { useEffect, useLayoutEffect, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';

export interface MenuItemDef {
  label?: ReactNode;
  icon?: ReactNode;
  hint?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onSelect?(): void;
  separator?: boolean;
  heading?: string;
}

interface MenuProps {
  trigger: (props: { onClick(e: React.MouseEvent): void; 'aria-expanded': boolean }) => ReactElement;
  items: MenuItemDef[] | (() => MenuItemDef[]);
  align?: 'left' | 'right';
}

/** Dropdown menu anchored to its trigger. Closes on outside click / Escape. */
export function Menu({ trigger, items, align = 'left' }: MenuProps) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  return (
    <>
      {trigger({
        onClick: (e) => {
          e.stopPropagation();
          setAnchor(anchor ? null : (e.currentTarget as HTMLElement).getBoundingClientRect());
        },
        'aria-expanded': !!anchor,
      })}
      {anchor && (
        <MenuPopup
          anchor={anchor}
          align={align}
          items={typeof items === 'function' ? items() : items}
          onClose={() => setAnchor(null)}
        />
      )}
    </>
  );
}

export function MenuPopup({
  anchor,
  items,
  onClose,
  align = 'left',
  point,
}: {
  anchor?: DOMRect;
  point?: { x: number; y: number };
  items: MenuItemDef[];
  onClose(): void;
  align?: 'left' | 'right';
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number }>({ left: -9999, top: -9999 });
  const [hl, setHl] = useState(-1);
  const actionable = items.map((it, i) => (!it.separator && !it.heading && !it.disabled ? i : -1)).filter((i) => i >= 0);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    let left = point ? point.x : align === 'right' && anchor ? anchor.right - w : (anchor?.left ?? 0);
    let top = point ? point.y : (anchor?.bottom ?? 0) + 4;
    if (left + w > window.innerWidth - 8) left = window.innerWidth - w - 8;
    if (top + h > window.innerHeight - 8) top = Math.max(8, (point ? point.y : (anchor?.top ?? 0)) - h - 4);
    setPos({ left: Math.max(8, left), top });
  }, [anchor, point, align]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const cur = actionable.indexOf(hl);
        const next = e.key === 'ArrowDown' ? (cur + 1) % actionable.length : (cur - 1 + actionable.length) % actionable.length;
        setHl(actionable[next] ?? -1);
      } else if (e.key === 'Enter' && hl >= 0) {
        e.preventDefault();
        items[hl]?.onSelect?.();
        onClose();
      }
    };
    const onScroll = () => onClose();
    // Deferred so the click that opened the menu does not close it. The timer must be
    // cancelled on cleanup: this effect re-runs on every render (and twice in StrictMode),
    // and a listener added after cleanup would leak and close the next popup on mousedown.
    const t = setTimeout(() => window.addEventListener('mousedown', onDown), 0);
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('resize', onScroll);
    return () => {
      clearTimeout(t);
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', onScroll);
    };
  }, [onClose, hl, items, actionable]);

  return createPortal(
    <div ref={ref} className="menu" style={pos} role="menu">
      {items.map((it, i) =>
        it.separator ? (
          <div key={i} className="menu-sep" />
        ) : it.heading ? (
          <div key={i} className="menu-label">
            {it.heading}
          </div>
        ) : (
          <button
            key={i}
            role="menuitem"
            className={clsx('menu-item', it.danger && 'danger', hl === i && 'hl')}
            disabled={it.disabled}
            onMouseEnter={() => setHl(i)}
            onClick={(e) => {
              e.stopPropagation();
              onClose();
              it.onSelect?.();
            }}
          >
            {it.icon}
            <span className="truncate">{it.label}</span>
            {it.hint && <span className="hint">{it.hint}</span>}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}

/** Hook for right-click context menus. */
export function useContextMenu() {
  const [state, setState] = useState<{ point: { x: number; y: number }; items: MenuItemDef[] } | null>(null);
  const open = (e: React.MouseEvent, items: MenuItemDef[]) => {
    e.preventDefault();
    e.stopPropagation();
    setState({ point: { x: e.clientX, y: e.clientY }, items });
  };
  const node = state ? <MenuPopup point={state.point} items={state.items} onClose={() => setState(null)} /> : null;
  return { open, node };
}
