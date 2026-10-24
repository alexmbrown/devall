import { useEffect, useMemo, useState } from 'react';
import { Activity, AlertTriangle, ShieldAlert } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Field, MethodBadge, Spinner, Switch } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { cloudApi, intervalLabel, MONITOR_INTERVALS, type MonitorCandidate, type MonitorDraft, type MonitorLimits, type MonitorView } from '@/lib/api/cloud';
import { useWorkspace } from '@/store/workspace';

/**
 * Create / edit a scheduled monitor: saved requests (resolved with one environment, secrets kept
 * as `{{secret.NAME}}`), the schedule, alerts — and, explicitly, which secret values to upload.
 */
export function MonitorDialog({ root, existing, limits, onClose, onSaved }: { root: string; existing: MonitorView | null; limits: MonitorLimits; onClose(): void; onSaved(): void }) {
  const project = useWorkspace((s) => s.project);
  const envs = project?.environments ?? [];
  const [name, setName] = useState(existing?.name ?? '');
  const [every, setEvery] = useState<number>(existing?.intervalMinutes ?? Math.max(limits.monitorMinIntervalMinutes, 15));
  const [env, setEnv] = useState<string>(() => useWorkspace.getState().activeEnv()?.id ?? envs[0]?.id ?? '');
  const [candidates, setCandidates] = useState<MonitorCandidate[] | null>(null);
  const [picked, setPicked] = useState<string[]>([]);
  const [draft, setDraft] = useState<MonitorDraft | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [upload, setUpload] = useState<string[]>([]);
  const [removeStored, setRemoveStored] = useState<string[]>([]);
  const [webhook, setWebhook] = useState(existing?.alerts.webhookUrl ?? '');
  const [slack, setSlack] = useState(existing?.alerts.slackWebhookUrl ?? '');
  const [onRecovery, setOnRecovery] = useState(existing?.alerts.onRecovery ?? true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    cloudApi.monitorCandidates(root).then(setCandidates).catch((e) => toast.error('Requests', errorMessage(e)));
  }, [root]);

  // Re-resolve whenever the selection or environment changes.
  useEffect(() => {
    if (!picked.length) {
      setDraft(null);
      return;
    }
    let cancelled = false;
    setPreparing(true);
    cloudApi
      .monitorPrepare(root, picked, env || undefined)
      .then((d) => {
        if (!cancelled) setDraft(d);
      })
      .catch((e) => toast.error('Monitor', errorMessage(e)))
      .finally(() => !cancelled && setPreparing(false));
    return () => {
      cancelled = true;
    };
  }, [root, picked, env]);

  const stored = existing?.secretNames ?? [];
  const neededSecrets = draft?.secretNames ?? (existing ? [...new Set(existing.requests.flatMap((r) => [...JSON.stringify(r).matchAll(/\{\{\s*secret\.([A-Za-z0-9_.-]+)\s*\}\}/g)].map((m) => m[1]!)))] : []);
  const missing = neededSecrets.filter((s) => !upload.includes(s) && !(stored.includes(s) && !removeStored.includes(s)));
  const tooMany = picked.length > limits.monitorMaxRequests;
  const requests = draft?.requests ?? (picked.length ? [] : (existing?.requests ?? []));
  const canSave = name.trim() && requests.length > 0 && !tooMany && !busy && !preparing;
  const allowedIntervals = useMemo(() => MONITOR_INTERVALS.filter((m) => m >= limits.monitorMinIntervalMinutes), [limits.monitorMinIntervalMinutes]);

  const save = async () => {
    if (upload.length && !limits.secretsAvailable) {
      toast.error('Monitor', "This relay can't store secrets (its operator hasn't set MONITOR_SECRET_KEY).");
      return;
    }
    setBusy(true);
    try {
      const input = {
        name: name.trim(),
        intervalMinutes: every,
        requests,
        alerts: { webhookUrl: webhook.trim() || null, slackWebhookUrl: slack.trim() || null, onRecovery },
        paused: existing?.paused ?? false,
      };
      await cloudApi.monitorSave(root, existing?.id, input, env || undefined, upload, removeStored);
      toast.success(existing ? 'Monitor updated' : 'Monitor created', `${input.name} · ${intervalLabel(every)}`);
      onSaved();
    } catch (e) {
      toast.error('Monitor', errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open
      size="lg"
      onClose={onClose}
      title={
        <span className="row-sm">
          <Activity size={16} /> {existing ? `Edit ${existing.name}` : 'New monitor'}
        </span>
      }
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!canSave} onClick={save}>
            {busy ? <Spinner /> : null} {existing ? 'Save' : 'Create monitor'}
          </button>
        </>
      }
    >
      <div className="col-lg">
        <div className="tn-form">
          <Field label="Name">
            <input className="input" value={name} placeholder="API health" onChange={(e) => setName(e.target.value)} autoFocus />
          </Field>
          <Field label="Every" hint={limits.monitorMinIntervalMinutes > 5 ? `Your plan: at most ${intervalLabel(limits.monitorMinIntervalMinutes)}` : undefined}>
            <select className="select" value={every} onChange={(e) => setEvery(Number(e.target.value))}>
              {allowedIntervals.map((m) => (
                <option key={m} value={m}>
                  {intervalLabel(m)}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Environment" hint="Variables are resolved now; secrets stay placeholders">
            <select className="select" value={env} onChange={(e) => setEnv(e.target.value)}>
              <option value="">(none)</option>
              {envs.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.file.name}
                </option>
              ))}
            </select>
          </Field>
        </div>

        <Field label={`Requests (${picked.length || (existing?.requests.length ?? 0)} / ${limits.monitorMaxRequests})`} hint={existing && !picked.length ? 'Keeping the current requests; pick to replace them.' : 'Their assertions decide up / down (none = any status below 400).'}>
          <div className="tn-picklist">
            {candidates === null ? (
              <Spinner />
            ) : candidates.length === 0 ? (
              <span className="xs subtle">No saved HTTP requests in this project.</span>
            ) : (
              candidates.map((c) => (
                <label key={c.path} className="row-sm tn-pick">
                  <Checkbox checked={picked.includes(c.path)} onChange={(v) => setPicked((p) => (v ? [...p, c.path] : p.filter((x) => x !== c.path)))} />
                  <MethodBadge method={c.method} short />
                  <span className="truncate">{c.name}</span>
                  <span className="xs subtle mono truncate">{c.path}</span>
                </label>
              ))
            )}
          </div>
        </Field>
        {tooMany && <Callout kind="warning">Your plan allows {limits.monitorMaxRequests} requests per monitor.</Callout>}
        {draft?.warnings.length ? (
          <Callout kind="warning">
            <ul className="tn-warnings">
              {draft.warnings.map(([p, w], i) => (
                <li key={i}>
                  <code className="code-inline">{p}</code> — {w}
                </li>
              ))}
            </ul>
          </Callout>
        ) : null}

        {neededSecrets.length > 0 && (
          <div className="tn-secrets">
            <div className="row-sm">
              <ShieldAlert size={16} className="text-warning" />
              <b>Secrets</b>
            </div>
            <p className="xs subtle">
              These requests use secrets. Nothing is uploaded unless you tick it: a ticked value leaves this machine, is stored on the relay encrypted with a key for your account, and is
              decrypted only while the monitor runs. Results never contain secret values. Prefer a dedicated, low-privilege token for monitoring.
            </p>
            {neededSecrets.map((s) => {
              const isStored = stored.includes(s) && !removeStored.includes(s);
              return (
                <div key={s} className="row-sm">
                  <Checkbox
                    checked={upload.includes(s)}
                    onChange={(v) => setUpload((u) => (v ? [...u, s] : u.filter((x) => x !== s)))}
                    label={
                      <>
                        Upload <code className="code-inline">{s}</code> from the {envs.find((e) => e.id === env)?.file.name ?? 'selected'} environment
                      </>
                    }
                  />
                  {isStored && !upload.includes(s) && <span className="badge outline">stored on the relay</span>}
                  {stored.includes(s) && (
                    <button className="btn btn-sm btn-ghost" onClick={() => setRemoveStored((r) => (r.includes(s) ? r.filter((x) => x !== s) : [...r, s]))}>
                      {removeStored.includes(s) ? 'Keep stored value' : 'Delete stored value'}
                    </button>
                  )}
                </div>
              );
            })}
            {missing.length > 0 && (
              <p className="xs text-warning row-sm">
                <AlertTriangle size={12} /> Without {missing.join(', ')} the checks that use {missing.length === 1 ? 'it' : 'them'} will fail.
              </p>
            )}
            {!limits.secretsAvailable && <Callout kind="danger">This relay can't store secrets (MONITOR_SECRET_KEY isn't configured).</Callout>}
          </div>
        )}

        <div className="tn-form">
          <Field label="Webhook (POST JSON)" hint="https:// — gets {monitor, status: down|up, failures}">
            <input className="input mono" value={webhook} placeholder="https://…" onChange={(e) => setWebhook(e.target.value)} />
          </Field>
          <Field label="Slack incoming webhook">
            <input className="input mono" value={slack} placeholder="https://hooks.slack.com/services/…" onChange={(e) => setSlack(e.target.value)} />
          </Field>
        </div>
        <Switch checked={onRecovery} onChange={setOnRecovery} label="Also alert when it recovers" />
      </div>
    </Modal>
  );
}
