import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import clsx from 'clsx';

/** An anchored floating panel (date picker, label picker…). Closes on outside click / Escape. */
export function Popover({ anchor, onClose, children, className, align = 'left' }: { anchor: DOMRect; onClose(): void; children: ReactNode; className?: string; align?: 'left' | 'right' }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: -9999, top: -9999 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    let left = align === 'right' ? anchor.right - w : anchor.left;
    let top = anchor.bottom + 4;
    if (left + w > window.innerWidth - 8) left = window.innerWidth - w - 8;
    if (top + h > window.innerHeight - 8) top = Math.max(8, anchor.top - h - 4);
    setPos({ left: Math.max(8, left), top });
  }, [anchor, align]);
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      // Menus opened from inside the popover are portalled elsewhere: keep the popover open.
      if (!ref.current?.contains(t) && !(t instanceof Element && t.closest('.menu'))) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        e.preventDefault();
        onClose();
      }
    };
    const t = setTimeout(() => window.addEventListener('mousedown', onDown), 0);
    window.addEventListener('keydown', onKey, true);
    return () => {
      clearTimeout(t);
      window.removeEventListener('mousedown', onDown);
      window.removeEventListener('keydown', onKey, true);
    };
  }, [onClose]);
  return createPortal(
    <div ref={ref} className={clsx('td-pop', className)} style={pos} role="dialog" onKeyDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
      {children}
    </div>,
    document.body,
  );
}

/** State for a popover toggled by a button: `{ anchor, toggle, close }`. */
export function usePopover() {
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  return {
    anchor,
    toggle: (e: React.MouseEvent) => {
      e.stopPropagation();
      setAnchor((a) => (a ? null : (e.currentTarget as HTMLElement).getBoundingClientRect()));
    },
    openAt: (el: HTMLElement) => setAnchor(el.getBoundingClientRect()),
    close: () => setAnchor(null),
  };
}
