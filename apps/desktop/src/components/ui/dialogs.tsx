import { useState, type ReactNode } from 'react';
import { create } from 'zustand';
import { Modal } from './Modal';

interface ConfirmOpts {
  title: string;
  message?: ReactNode;
  confirmText?: string;
  danger?: boolean;
  /** When set, the user must type this text to enable the confirm button. */
  typeToConfirm?: string;
}
interface PromptOpts {
  title: string;
  label?: string;
  initial?: string;
  placeholder?: string;
  confirmText?: string;
  validate?(v: string): string | undefined;
}

export type SaveChoice = 'save' | 'discard' | 'cancel';

interface SaveOpts {
  title: string;
  message?: ReactNode;
}

type Pending =
  | { kind: 'confirm'; opts: ConfirmOpts; resolve(v: boolean): void }
  | { kind: 'prompt'; opts: PromptOpts; resolve(v: string | null): void }
  | { kind: 'save'; opts: SaveOpts; resolve(v: SaveChoice): void };

const useDialogs = create<{ pending?: Pending; set(p?: Pending): void }>((set) => ({
  pending: undefined,
  set: (pending) => set({ pending }),
}));

export function confirmDialog(opts: ConfirmOpts): Promise<boolean> {
  return new Promise((resolve) => useDialogs.getState().set({ kind: 'confirm', opts, resolve }));
}

export function promptDialog(opts: PromptOpts): Promise<string | null> {
  return new Promise((resolve) => useDialogs.getState().set({ kind: 'prompt', opts, resolve }));
}

/** "Save changes?" with Save / Don't save / Cancel (Escape cancels). */
export function saveChangesDialog(opts: SaveOpts): Promise<SaveChoice> {
  return new Promise((resolve) => useDialogs.getState().set({ kind: 'save', opts, resolve }));
}

export function DialogHost() {
  const { pending, set } = useDialogs();
  if (!pending) return null;
  const close = (v: boolean | string | null) => {
    set(undefined);
    (pending.resolve as (x: unknown) => void)(v);
  };
  if (pending.kind === 'confirm') return <ConfirmView opts={pending.opts} close={close} />;
  if (pending.kind === 'save') return <SaveView opts={pending.opts} close={(v) => close(v)} />;
  return <PromptView opts={pending.opts} close={close} />;
}

function ConfirmView({ opts, close }: { opts: ConfirmOpts; close(v: boolean): void }) {
  const [typed, setTyped] = useState('');
  const blocked = !!opts.typeToConfirm && typed !== opts.typeToConfirm;
  return (
    <Modal
      open
      onClose={() => close(false)}
      title={opts.title}
      footer={
        <>
          <button className="btn" onClick={() => close(false)}>
            Cancel
          </button>
          <button
            className={opts.danger ? 'btn btn-danger' : 'btn btn-primary'}
            disabled={blocked}
            onClick={() => close(true)}
            autoFocus={!opts.typeToConfirm}
          >
            {opts.confirmText ?? 'Confirm'}
          </button>
        </>
      }
    >
      <div className="col">
        {opts.message && <div className="muted">{opts.message}</div>}
        {opts.typeToConfirm && (
          <div className="field">
            <label>
              Type <code className="code-inline">{opts.typeToConfirm}</code> to confirm
            </label>
            <input
              className="input"
              autoFocus
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && !blocked && close(true)}
            />
          </div>
        )}
      </div>
    </Modal>
  );
}

function SaveView({ opts, close }: { opts: SaveOpts; close(v: SaveChoice | null): void }) {
  return (
    <Modal
      open
      onClose={() => close('cancel')}
      title={opts.title}
      footer={
        <>
          <button className="btn" onClick={() => close('cancel')}>
            Cancel
          </button>
          <span className="spacer" />
          <button className="btn" onClick={() => close('discard')}>
            Don’t save
          </button>
          <button className="btn btn-primary" onClick={() => close('save')} autoFocus>
            Save
          </button>
        </>
      }
    >
      {opts.message && <div className="muted">{opts.message}</div>}
    </Modal>
  );
}

function PromptView({ opts, close }: { opts: PromptOpts; close(v: string | null): void }) {
  const [value, setValue] = useState(opts.initial ?? '');
  const error = opts.validate?.(value);
  const submit = () => !error && close(value);
  return (
    <Modal
      open
      onClose={() => close(null)}
      title={opts.title}
      footer={
        <>
          <button className="btn" onClick={() => close(null)}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!!error} onClick={submit}>
            {opts.confirmText ?? 'OK'}
          </button>
        </>
      }
    >
      <div className="field">
        {opts.label && <label>{opts.label}</label>}
        <input
          className="input"
          autoFocus
          value={value}
          placeholder={opts.placeholder}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && submit()}
          onFocus={(e) => e.currentTarget.select()}
        />
        {error && value && <div className="error">{error}</div>}
      </div>
    </Modal>
  );
}
