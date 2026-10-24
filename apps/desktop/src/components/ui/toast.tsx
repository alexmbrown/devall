import { create } from 'zustand';
import { CheckCircle2, Info, XCircle, X } from 'lucide-react';
import { uid } from '@/lib/id';

type Kind = 'success' | 'error' | 'info';
interface Toast {
  id: string;
  kind: Kind;
  title: string;
  detail?: string;
  action?: { label: string; run: () => void };
}

interface ToastState {
  toasts: Toast[];
  push(t: Omit<Toast, 'id'>, ms?: number): void;
  dismiss(id: string): void;
}

const useToasts = create<ToastState>((set, get) => ({
  toasts: [],
  push(t, ms = 3800) {
    const id = uid();
    set((s) => ({ toasts: [...s.toasts.slice(-4), { ...t, id }] }));
    if (ms > 0) setTimeout(() => get().dismiss(id), ms);
  },
  dismiss(id) {
    set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) }));
  },
}));

export const toast = {
  success: (title: string, detail?: string, action?: Toast['action']) =>
    useToasts.getState().push({ kind: 'success', title, detail, action }, action ? 7000 : 3200),
  error: (title: string, detail?: unknown) =>
    useToasts.getState().push(
      { kind: 'error', title, detail: detail instanceof Error ? detail.message : detail ? String(detail) : undefined },
      7000,
    ),
  info: (title: string, detail?: string, action?: Toast['action']) => useToasts.getState().push({ kind: 'info', title, detail, action }, action ? 12000 : undefined),
};

const ICON = { success: CheckCircle2, error: XCircle, info: Info };

export function Toaster() {
  const { toasts, dismiss } = useToasts();
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((t) => {
        const Icon = ICON[t.kind];
        return (
          <div key={t.id} className={`toast ${t.kind}`}>
            <Icon />
            <div className="grow">
              <div className="strong">{t.title}</div>
              {t.detail && <div className="muted small break">{t.detail}</div>}
            </div>
            {t.action && (
              <button
                className="btn btn-sm"
                onClick={() => {
                  t.action!.run();
                  dismiss(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
            <button className="icon-btn sm" onClick={() => dismiss(t.id)} aria-label="Dismiss">
              <X />
            </button>
          </div>
        );
      })}
    </div>
  );
}
