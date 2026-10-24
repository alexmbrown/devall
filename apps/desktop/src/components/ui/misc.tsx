import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { Check, Copy, Info, AlertTriangle, XCircle, CheckCircle2, Search, X } from 'lucide-react';
import { toast } from './toast';

const SHORT: Record<string, string> = { OPTIONS: 'OPT', DELETE: 'DEL' };

export function MethodBadge({ method, short }: { method?: string; short?: boolean }) {
  const m = (method || 'GET').toUpperCase();
  const cls = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].includes(m) ? `method-${m}` : 'method-other';
  return (
    <span className={clsx('badge method', cls)} title={m}>
      {short ? (SHORT[m] ?? m) : m === 'OPTIONS' ? 'OPT' : m}
    </span>
  );
}

export function StatusCode({ status }: { status?: number }) {
  if (status === undefined) return <span className="status-code subtle">—</span>;
  return <span className={clsx('status-code', `status-${String(status)[0]}`)}>{status}</span>;
}

export function Spinner({ className }: { className?: string }) {
  return <span className={clsx('spinner', className)} aria-label="Loading" />;
}

export function Empty({ icon, title, children, action }: { icon?: ReactNode; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty">
      {icon}
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

export function Callout({ kind = 'info', children, className }: { kind?: 'info' | 'warning' | 'danger' | 'success'; children: ReactNode; className?: string }) {
  const Icon = { info: Info, warning: AlertTriangle, danger: XCircle, success: CheckCircle2 }[kind];
  return (
    <div className={clsx('callout', kind !== 'info' && kind, className)}>
      <Icon />
      <div className="grow">{children}</div>
    </div>
  );
}

export function Switch({ checked, onChange, label, disabled, title }: { checked: boolean; onChange(v: boolean): void; label?: ReactNode; disabled?: boolean; title?: string }) {
  return (
    <label className="switch" title={title} style={disabled ? { opacity: 0.5 } : undefined}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span className="track" />
      {label && <span>{label}</span>}
    </label>
  );
}

export function Checkbox({ checked, onChange, label, indeterminate, title }: { checked: boolean; onChange(v: boolean): void; label?: ReactNode; indeterminate?: boolean; title?: string }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = !!indeterminate;
  }, [indeterminate]);
  return (
    <label className="check" title={title} onClick={(e) => e.stopPropagation()}>
      <input ref={ref} type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      {label && <span>{label}</span>}
    </label>
  );
}

export function Segmented<T extends string>({ value, onChange, options, className }: { value: T; onChange(v: NoInfer<T>): void; options: { value: NoInfer<T>; label: ReactNode; title?: string }[]; className?: string }) {
  return (
    <div className={clsx('segmented', className)} role="tablist">
      {options.map((o) => (
        <button key={o.value} className={clsx(o.value === value && 'on')} onClick={() => onChange(o.value)} title={o.title} role="tab" aria-selected={o.value === value}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Field({ label, hint, error, children, className }: { label?: ReactNode; hint?: ReactNode; error?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={clsx('field', className)}>
      {label && <label>{label}</label>}
      {children}
      {error ? <div className="error">{error}</div> : hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

export function SearchInput({ value, onChange, placeholder, className, autoFocus, inputRef }: { value: string; onChange(v: string): void; placeholder?: string; className?: string; autoFocus?: boolean; inputRef?: React.Ref<HTMLInputElement> }) {
  return (
    <div className={clsx('input-group', className)}>
      <Search />
      <input ref={inputRef} className="input sm" value={value} placeholder={placeholder ?? 'Search…'} onChange={(e) => onChange(e.target.value)} autoFocus={autoFocus} onKeyDown={(e) => e.key === 'Escape' && onChange('')} />
      {value && (
        <button className="icon-btn sm input-suffix" onClick={() => onChange('')} aria-label="Clear search">
          <X />
        </button>
      )}
    </div>
  );
}

export async function copyText(text: string, what = 'Copied to clipboard') {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(what);
  } catch {
    toast.error('Clipboard unavailable', 'The browser refused clipboard access.');
  }
}

export function CopyButton({ text, label, className, title = 'Copy', onCopy }: { text: string | (() => string); label?: string; className?: string; title?: string; onCopy?(): void }) {
  const [done, setDone] = useState(false);
  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(typeof text === 'function' ? text() : text);
      onCopy?.();
      setDone(true);
      setTimeout(() => setDone(false), 1200);
    } catch {
      toast.error('Clipboard unavailable');
    }
  };
  if (label)
    return (
      <button className={clsx('btn btn-sm', className)} onClick={onClick} title={title}>
        {done ? <Check /> : <Copy />} {label}
      </button>
    );
  return (
    <button className={clsx('icon-btn sm', className)} onClick={onClick} title={title} aria-label={title}>
      {done ? <Check /> : <Copy />}
    </button>
  );
}

/**
 * Drag handle between two panes. `axis="x"` (default) reports the new width of the pane to its left,
 * `axis="y"` the new height of the pane above it. Double-click restores `defaultValue`. `invert`
 * sizes the pane after the handle instead (right-side panels: dragging left makes them wider).
 */
export function Resizer({
  width,
  onChange,
  min = 200,
  max = 800,
  defaultValue = 340,
  axis = 'x',
  invert,
  className,
}: {
  width: number;
  onChange(w: number): void;
  min?: number;
  max?: number;
  defaultValue?: number;
  axis?: 'x' | 'y';
  invert?: boolean;
  className?: string;
}) {
  const [drag, setDrag] = useState(false);
  const start = useRef({ pos: 0, w: 0 });
  const onMove = useCallback(
    (e: PointerEvent) => {
      const delta = (axis === 'x' ? e.clientX : e.clientY) - start.current.pos;
      onChange(Math.round(Math.min(max, Math.max(min, start.current.w + (invert ? -delta : delta)))));
    },
    [onChange, min, max, axis, invert],
  );
  useEffect(() => {
    if (!drag) return;
    const up = () => setDrag(false);
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', up);
    document.body.style.cursor = axis === 'x' ? 'col-resize' : 'row-resize';
    document.body.style.userSelect = 'none';
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', up);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
  }, [drag, onMove, axis]);
  return (
    <div
      className={clsx('resizer', axis === 'y' && 'resizer-y', drag && 'dragging', className)}
      onPointerDown={(e) => {
        e.preventDefault();
        start.current = { pos: axis === 'x' ? e.clientX : e.clientY, w: width };
        setDrag(true);
      }}
      onDoubleClick={() => onChange(Math.min(max, Math.max(min, defaultValue)))}
      role="separator"
      aria-orientation={axis === 'x' ? 'vertical' : 'horizontal'}
    />
  );
}

export function RelTime({ ts }: { ts?: number | string }) {
  const [, force] = useState(0);
  useEffect(() => {
    const t = setInterval(() => force((x) => x + 1), 15000);
    return () => clearInterval(t);
  }, []);
  if (!ts) return null;
  const d = typeof ts === 'number' ? new Date(ts) : new Date(ts);
  return (
    <time dateTime={d.toISOString()} title={d.toLocaleString()}>
      {relTime(d.getTime())}
    </time>
  );
}

export function relTime(t: number): string {
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 5) return 'just now';
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  return new Date(t).toLocaleDateString();
}

export function formatTime(t?: number): string {
  if (!t) return '';
  const d = new Date(t);
  return `${d.toLocaleTimeString([], { hour12: false })}.${String(d.getMilliseconds()).padStart(3, '0')}`;
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

export const isMac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
export const MOD = isMac ? '⌘' : 'Ctrl';
