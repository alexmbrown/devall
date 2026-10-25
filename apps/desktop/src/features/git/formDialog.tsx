import { useState, type ReactNode } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { FolderOpen } from 'lucide-react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Modal } from '@/components/ui/Modal';
import { Checkbox, Field } from '@/components/ui/misc';

/**
 * Small forms for Source control (new branch, merge mode, reset mode, stash options, remotes,
 * worktrees…): `formDialog({ title, fields })` resolves with the values, or null when cancelled.
 * One host (`GitFormHost`) renders them. Destructive forms can require typing a word first.
 */

export type FormValues = Record<string, string | boolean>;

export type FormField =
  | { kind: 'text'; key: string; label: string; initial?: string; placeholder?: string; mono?: boolean; password?: boolean; multiline?: boolean; hint?: ReactNode; validate?(v: string, all: FormValues): string | undefined; show?(all: FormValues): boolean }
  | { kind: 'path'; key: string; label: string; initial?: string; hint?: ReactNode; validate?(v: string, all: FormValues): string | undefined; show?(all: FormValues): boolean }
  | { kind: 'check'; key: string; label: ReactNode; initial?: boolean; hint?: ReactNode; show?(all: FormValues): boolean }
  | { kind: 'choice'; key: string; label?: string; initial: string; options: { value: string; label: ReactNode; hint?: ReactNode; danger?: boolean }[]; show?(all: FormValues): boolean }
  | { kind: 'select'; key: string; label: string; initial: string; options: { value: string; label: string }[]; show?(all: FormValues): boolean }
  | { kind: 'note'; content: ReactNode | ((all: FormValues) => ReactNode); show?(all: FormValues): boolean };

export interface FormOpts {
  title: ReactNode;
  fields: FormField[];
  confirmText?: string | ((all: FormValues) => string);
  danger?: boolean | ((all: FormValues) => boolean);
  /** The user types this word to enable the confirm button (when it returns a string). */
  typeToConfirm?(all: FormValues): string | undefined;
  size?: 'md' | 'lg';
}

type Pending = { id: number; opts: FormOpts; resolve(v: FormValues | null): void };
let nextId = 0;

const useForms = create<{ pending: Pending | null; set(p: Pending | null): void }>((set) => ({ pending: null, set: (pending) => set({ pending }) }));

export function formDialog(opts: FormOpts): Promise<FormValues | null> {
  return new Promise((resolve) => useForms.getState().set({ id: ++nextId, opts, resolve }));
}

const initialValues = (fields: FormField[]): FormValues => {
  const v: FormValues = {};
  for (const f of fields) {
    if (f.kind === 'text' || f.kind === 'path') v[f.key] = f.initial ?? '';
    else if (f.kind === 'check') v[f.key] = !!f.initial;
    else if (f.kind === 'choice' || f.kind === 'select') v[f.key] = f.initial;
  }
  return v;
};

export function GitFormHost() {
  const pending = useForms((s) => s.pending);
  if (!pending) return null;
  return <FormView key={pending.id} pending={pending} />;
}

function FormView({ pending }: { pending: Pending }) {
  const { opts } = pending;
  const [values, setValues] = useState<FormValues>(() => initialValues(opts.fields));
  const [typed, setTyped] = useState('');
  const close = (v: FormValues | null) => {
    useForms.getState().set(null);
    pending.resolve(v);
  };
  const visible = opts.fields.filter((f) => !f.show || f.show(values));
  const errors = visible.map((f) => ((f.kind === 'text' || f.kind === 'path') && f.validate ? f.validate(String(values[f.key] ?? ''), values) : undefined));
  const word = opts.typeToConfirm?.(values);
  const danger = typeof opts.danger === 'function' ? opts.danger(values) : !!opts.danger;
  const ok = errors.every((e) => !e) && (!word || typed.trim() === word);
  const confirmText = typeof opts.confirmText === 'function' ? opts.confirmText(values) : (opts.confirmText ?? 'OK');
  const setV = (k: string, v: string | boolean) => setValues((x) => ({ ...x, [k]: v }));
  return (
    <Modal
      open
      onClose={() => close(null)}
      title={opts.title}
      size={opts.size}
      footer={
        <>
          <button className="btn" onClick={() => close(null)}>
            Cancel
          </button>
          <button className={clsx('btn', danger ? 'btn-danger' : 'btn-primary')} disabled={!ok} onClick={() => close(values)}>
            {confirmText}
          </button>
        </>
      }
    >
      <form
        className="col"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) close(values);
        }}
      >
        {visible.map((f, i) => {
          const err = errors[i];
          switch (f.kind) {
            case 'note':
              return <div key={i} className="small">{typeof f.content === 'function' ? f.content(values) : f.content}</div>;
            case 'text':
              return (
                <Field key={f.key} label={f.label} hint={f.hint} error={String(values[f.key] ?? '') ? err : undefined}>
                  {f.multiline ? (
                    <textarea className="textarea" rows={4} value={String(values[f.key] ?? '')} placeholder={f.placeholder} onChange={(e) => setV(f.key, e.target.value)} />
                  ) : (
                    <input className={clsx('input', f.mono && 'mono')} type={f.password ? 'password' : 'text'} autoComplete="off" spellCheck={false} value={String(values[f.key] ?? '')} placeholder={f.placeholder} autoFocus={i === visible.findIndex((x) => x.kind === 'text')} onChange={(e) => setV(f.key, e.target.value)} />
                  )}
                </Field>
              );
            case 'path':
              return (
                <Field key={f.key} label={f.label} hint={f.hint} error={err}>
                  <div className="row-sm">
                    <input className="input mono grow" value={String(values[f.key] ?? '')} onChange={(e) => setV(f.key, e.target.value)} />
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={async () => {
                        const dir = await openDialog({ directory: true, title: f.label });
                        if (typeof dir === 'string') setV(f.key, dir);
                      }}
                    >
                      <FolderOpen /> Browse…
                    </button>
                  </div>
                </Field>
              );
            case 'check':
              return (
                <div key={f.key} className="col-sm">
                  <Checkbox checked={!!values[f.key]} onChange={(v) => setV(f.key, v)} label={<span className="small">{f.label}</span>} />
                  {f.hint && <span className="xs subtle git-form-hint">{f.hint}</span>}
                </div>
              );
            case 'select':
              return (
                <Field key={f.key} label={f.label}>
                  <select className="select" value={String(values[f.key])} onChange={(e) => setV(f.key, e.target.value)}>
                    {f.options.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </Field>
              );
            case 'choice':
              return (
                <div key={f.key} className="col-sm" role="radiogroup" aria-label={f.label}>
                  {f.label && <span className="small strong">{f.label}</span>}
                  {f.options.map((o) => (
                    <label key={o.value} className={clsx('git-choice', values[f.key] === o.value && 'on', o.danger && 'danger')}>
                      <input type="radio" name={f.key} checked={values[f.key] === o.value} onChange={() => setV(f.key, o.value)} />
                      <span className="col-xs">
                        <span className="small strong">{o.label}</span>
                        {o.hint && <span className="xs subtle">{o.hint}</span>}
                      </span>
                    </label>
                  ))}
                </div>
              );
          }
        })}
        {word && (
          <Field label={<span>Type <span className="mono strong">{word}</span> to confirm</span>}>
            <input className="input mono" autoComplete="off" spellCheck={false} value={typed} onChange={(e) => setTyped(e.target.value)} />
          </Field>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
