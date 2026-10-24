import { useEffect, useState } from 'react';
import { History, Save } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type Retention } from '@/lib/api/webhooks';

export function retentionLabel(r?: Retention | null) {
  if (!r || (!r.maxEvents && !r.maxAgeDays)) return 'unlimited';
  return [r.maxEvents ? `${r.maxEvents} events` : '', r.maxAgeDays ? `${r.maxAgeDays} days` : ''].filter(Boolean).join(' / ');
}

const num = (s: string) => {
  const n = Number(s.replace(/\D/g, ''));
  return n > 0 ? n : null;
};

/** History is unlimited by default; optionally keep only the newest N events and/or the last N days. */
export function RetentionDialog({ root, onClose, onSaved }: { root: string; onClose(): void; onSaved(r: Retention): void }) {
  const [maxEvents, setMaxEvents] = useState('');
  const [maxAge, setMaxAge] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    webhooksApi
      .retention(root)
      .then((r) => {
        setMaxEvents(r.maxEvents ? String(r.maxEvents) : '');
        setMaxAge(r.maxAgeDays ? String(r.maxAgeDays) : '');
      })
      .catch(() => {});
  }, [root]);

  const save = async () => {
    setBusy(true);
    try {
      const r: Retention = { maxEvents: num(maxEvents), maxAgeDays: num(maxAge) };
      const removed = await webhooksApi.setRetention(root, r);
      toast.success(`History: ${retentionLabel(r)}`, removed ? `${removed} older event(s) removed` : undefined);
      onSaved(r);
      onClose();
    } catch (e) {
      toast.error('Could not save retention', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      onClose={onClose}
      title={
        <>
          <History size={16} /> Webhook history
        </>
      }
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={save} disabled={busy}>
            {busy ? <Spinner /> : <Save />} Save
          </button>
        </>
      }
    >
      <div className="wh-dialog">
        <p className="small muted">
          Events are kept on this machine (the project data folder, never in the project) — unlimited by default. Leave a field empty for no limit.
        </p>
        <div className="grid-2">
          <Field label="Keep the newest events">
            <input className="input mono" inputMode="numeric" value={maxEvents} placeholder="unlimited" onChange={(e) => setMaxEvents(e.target.value.replace(/\D/g, ''))} />
          </Field>
          <Field label="Keep events for (days)">
            <input className="input mono" inputMode="numeric" value={maxAge} placeholder="unlimited" onChange={(e) => setMaxAge(e.target.value.replace(/\D/g, ''))} />
          </Field>
        </div>
      </div>
    </Modal>
  );
}
