import { useEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';
import clsx from 'clsx';

interface ModalProps {
  open: boolean;
  onClose(): void;
  title?: ReactNode;
  size?: 'md' | 'lg' | 'xl';
  children: ReactNode;
  footer?: ReactNode;
  headerExtra?: ReactNode;
  className?: string;
  bodyClassName?: string;
}

export function Modal({ open, onClose, title, size = 'md', children, footer, headerExtra, className, bodyClassName }: ModalProps) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    const prev = document.activeElement as HTMLElement | null;
    requestAnimationFrame(() => {
      const el = ref.current?.querySelector<HTMLElement>('[autofocus], input, select, textarea, button.btn-primary');
      el?.focus();
    });
    return () => {
      window.removeEventListener('keydown', onKey, true);
      prev?.focus?.();
    };
  }, [open, onClose]);
  if (!open) return null;
  return createPortal(
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div ref={ref} className={clsx('modal', size !== 'md' && size, className)} role="dialog" aria-modal="true">
        {title !== undefined && (
          <div className="modal-header">
            <h2 className="grow truncate">{title}</h2>
            {headerExtra}
            <button className="icon-btn" onClick={onClose} aria-label="Close">
              <X />
            </button>
          </div>
        )}
        <div className={clsx('modal-body', bodyClassName)}>{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>,
    document.body,
  );
}
