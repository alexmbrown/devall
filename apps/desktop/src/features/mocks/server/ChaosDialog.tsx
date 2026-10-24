import { useEffect, useState } from 'react';
import { Flame } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, Spinner, Switch } from '@/features/mocks/components/ui';
import { useChaos, useWrite, qk } from '@/features/mocks/lib/queries';
import type { ChaosConfig, ChaosFault } from '@/features/mocks/lib/api';
import { FAULTS } from '@/features/mocks/stubs/form/ResponseSection';
import { TagsInput } from '@/features/mocks/stubs/form/PatternEditors';

const DEFAULT: ChaosConfig = { enabled: false, latencyMs: 0, jitterMs: 0, errorRate: 0, errorStatuses: [500, 502, 503, 504], faultRate: 0, faults: FAULTS.map((f) => f.value), urlPattern: undefined };

const PRESETS: { label: string; c: Partial<ChaosConfig> }[] = [
  { label: 'Slow network', c: { enabled: true, latencyMs: 800, jitterMs: 600, errorRate: 0, faultRate: 0 } },
  { label: 'Flaky backend (10% 5xx)', c: { enabled: true, latencyMs: 50, jitterMs: 150, errorRate: 0.1, faultRate: 0 } },
  { label: 'Broken connections (5%)', c: { enabled: true, latencyMs: 0, jitterMs: 0, errorRate: 0, faultRate: 0.05 } },
  { label: 'Everything at once', c: { enabled: true, latencyMs: 300, jitterMs: 700, errorRate: 0.1, faultRate: 0.05 } },
];

const pct = (r: number) => Math.round(r * 1000) / 10;

/** Project-wide chaos for the /dev/all engine (`GET/PUT /__admin/devall/chaos`). */
export function ChaosDialog({ onClose }: { onClose(): void }) {
  const q = useChaos(true);
  const write = useWrite();
  const [c, setC] = useState<ChaosConfig | null>(null);
  useEffect(() => {
    if (q.data && !c) setC({ ...DEFAULT, ...q.data });
  }, [q.data, c]);
  const set = (patch: Partial<ChaosConfig>) => c && setC({ ...c, ...patch });
  const apply = async (next: ChaosConfig) => {
    const body = { ...next, urlPattern: next.urlPattern?.trim() || undefined };
    const r = await write((w) => w.setChaos(body), { label: 'Update chaos', success: next.enabled ? 'Chaos on' : 'Chaos off', invalidate: (sid) => [qk.chaos(sid)] }).catch(() => undefined);
    if (r) onClose();
  };

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      title={
        <span className="row-sm">
          <Flame size={16} /> Chaos
        </span>
      }
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!c} onClick={() => c && apply(c)}>
            Apply
          </button>
        </>
      }
    >
      {q.isError ? (
        <Callout kind="danger">{q.error?.message}</Callout>
      ) : !c ? (
        <Spinner />
      ) : (
        <div className="col">
          <div className="subtle small">
            Applies to every mock request on this server, on top of per-stub delays and faults — test timeouts, retries and error handling without touching the stubs. Journal entries
            show what chaos did.
          </div>
          <div className="row wrap">
            {PRESETS.map((p) => (
              <button key={p.label} className="btn btn-sm" onClick={() => setC({ ...c, ...p.c })}>
                {p.label}
              </button>
            ))}
            <button className="btn btn-sm btn-ghost" onClick={() => setC({ ...DEFAULT })}>
              Reset
            </button>
          </div>
          <Switch checked={c.enabled} onChange={(enabled) => set({ enabled })} label={<span className="strong">Chaos enabled</span>} />
          <div className="grid-2">
            <Field label="Added latency (ms)">
              <input className="input" type="number" min={0} value={c.latencyMs} onChange={(e) => set({ latencyMs: Math.max(0, Number(e.target.value) || 0) })} />
            </Field>
            <Field label="Random extra latency up to (ms)">
              <input className="input" type="number" min={0} value={c.jitterMs} onChange={(e) => set({ jitterMs: Math.max(0, Number(e.target.value) || 0) })} />
            </Field>
            <Field label={`Error rate: ${pct(c.errorRate)}%`} hint="Answer with one of the statuses below instead of the stub">
              <input type="range" min={0} max={1} step={0.01} value={c.errorRate} onChange={(e) => set({ errorRate: Number(e.target.value) })} />
            </Field>
            <Field label="Error statuses">
              <TagsInput
                value={c.errorStatuses.map(String)}
                onChange={(v) => set({ errorStatuses: v.map(Number).filter((n) => Number.isInteger(n) && n >= 100 && n <= 599) })}
                placeholder="503, Enter to add"
              />
            </Field>
            <Field label={`Fault rate: ${pct(c.faultRate)}%`} hint="Break the connection instead of answering">
              <input type="range" min={0} max={1} step={0.01} value={c.faultRate} onChange={(e) => set({ faultRate: Number(e.target.value) })} />
            </Field>
            <Field label="Faults">
              <div className="col-sm">
                {FAULTS.map((f) => (
                  <Checkbox
                    key={f.value}
                    checked={c.faults.includes(f.value as ChaosFault)}
                    onChange={(v) => set({ faults: v ? [...c.faults, f.value as ChaosFault] : c.faults.filter((x) => x !== f.value) })}
                    label={<span title={f.help}>{f.label}</span>}
                  />
                ))}
              </div>
            </Field>
          </div>
          <Field label="Only URLs matching (regex, optional)" hint="Path + query must match fully, e.g. /api/payments/.*">
            <input className="input mono" value={c.urlPattern ?? ''} placeholder="(all requests)" onChange={(e) => set({ urlPattern: e.target.value || undefined })} />
          </Field>
          {c.errorRate > 0 && !c.errorStatuses.length && <Callout kind="warning">Add at least one error status.</Callout>}
          {c.faultRate > 0 && !c.faults.length && <Callout kind="warning">Pick at least one fault.</Callout>}
        </div>
      )}
    </Modal>
  );
}
