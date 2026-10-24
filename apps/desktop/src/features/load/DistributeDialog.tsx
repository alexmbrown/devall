import { useEffect, useState } from 'react';
import { CheckCircle2, Network, Plus, ShieldAlert, X, XCircle } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, CopyButton, Field, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { loadApi, type AgentCheck, type AgentTarget, type DistributedRun, type Preflight } from '@/lib/api/load';

const AGENTS_KEY = 'devall.load.agents';

function loadAgents(): AgentTarget[] {
  try {
    const v = JSON.parse(localStorage.getItem(AGENTS_KEY) ?? '[]') as AgentTarget[];
    if (Array.isArray(v) && v.length) return v.map((a) => ({ address: String(a.address ?? ''), weight: Math.max(1, Number(a.weight) || 1) }));
  } catch {
    /* default below */
  }
  return [{ address: '', weight: 1 }];
}

interface Props {
  open: boolean;
  onClose(): void;
  /** The test's current preflight (secrets used, target hosts). */
  preflight: Preflight | null;
  onRun(d: DistributedRun): void;
}

/** "Distribute across agents…": where the load comes from, the agents' token and consent to send secrets. */
export function DistributeDialog({ open, onClose, preflight, onRun }: Props) {
  const [agents, setAgents] = useState<AgentTarget[]>(loadAgents);
  const [token, setToken] = useState('');
  const [remember, setRemember] = useState(true);
  const [stored, setStored] = useState(false);
  const [allowSecrets, setAllowSecrets] = useState(false);
  const [checks, setChecks] = useState<AgentCheck[] | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    if (!open) return;
    setChecks(null);
    setAllowSecrets(false);
    loadApi
      .agentsToken()
      .then(setStored)
      .catch(() => setStored(false));
  }, [open]);

  const valid = agents.filter((a) => a.address.trim());
  const secrets = preflight?.secretRequests ?? [];
  const needsConsent = secrets.length > 0 && !allowSecrets;
  const noToken = !token.trim() && !stored;
  const set = (i: number, patch: Partial<AgentTarget>) => setAgents((list) => list.map((a, j) => (j === i ? { ...a, ...patch } : a)));
  const persist = () => {
    try {
      localStorage.setItem(AGENTS_KEY, JSON.stringify(valid));
    } catch {
      /* per-device convenience only */
    }
  };

  const check = async () => {
    setChecking(true);
    try {
      setChecks(await loadApi.agentsCheck(valid, token.trim() || undefined));
    } catch (e) {
      toast.error('Could not check the agents', errorMessage(e));
    } finally {
      setChecking(false);
    }
  };

  const run = () => {
    persist();
    onRun({ agents: valid.map((a) => ({ address: a.address.trim(), weight: Math.max(1, Math.round(a.weight)) })), token: token.trim() || undefined, rememberToken: remember && !!token.trim(), allowSecrets });
    onClose();
  };

  const total = valid.reduce((n, a) => n + Math.max(1, a.weight), 0);

  return (
    <Modal
      open={open}
      onClose={onClose}
      size="lg"
      title="Distribute across load agents"
      footer={
        <>
          <button className="btn" onClick={() => void check()} disabled={checking || !valid.length || noToken}>
            {checking ? <Spinner /> : <Network />} Check agents
          </button>
          <span className="grow" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" onClick={run} disabled={!valid.length || noToken || needsConsent} title={needsConsent ? 'Allow sending resolved secrets first' : undefined}>
            Run on {valid.length || 'no'} agent{valid.length === 1 ? '' : 's'}
          </button>
        </>
      }
    >
      <div className="col">
        <div className="subtle small">
          Start an agent on each machine with <code className="code-inline">DEVALL_AGENT_TOKEN=… devall load agent</code> (or the Docker image / <code className="code-inline">devall load agent k8s</code>). The
          arrival rate (or VUs) is split by weight; results merge exactly and thresholds apply to the merged run.
        </div>
        <Field label="Agents" hint="host:port (default port 7777), https://… for TLS agents, or dns:<headless-service>:7777 for every pod of a Kubernetes Service">
          <div className="col-sm">
            {agents.map((a, i) => {
              const c = checks?.find((x) => x.address === a.address.trim());
              return (
                <div key={i} className="row-sm">
                  <input className="input sm mono grow" value={a.address} onChange={(e) => set(i, { address: e.target.value })} placeholder="10.0.0.12:7777" autoFocus={i === 0} />
                  <input className="input sm mono" type="number" min={1} style={{ width: 70 }} value={a.weight} onChange={(e) => set(i, { weight: Number(e.target.value) || 1 })} title="Weight (relative share of the load)" aria-label="Weight" />
                  <span className="subtle xs nowrap" style={{ width: 44, textAlign: 'right' }}>
                    {a.address.trim() && total ? `${Math.round((Math.max(1, a.weight) / total) * 100)}%` : ''}
                  </span>
                  {c && (c.error ? <XCircle size={14} className="text-danger" /> : <CheckCircle2 size={14} className="text-success" />)}
                  <button className="icon-btn sm" onClick={() => setAgents((l) => (l.length > 1 ? l.filter((_, j) => j !== i) : [{ address: '', weight: 1 }]))} aria-label="Remove agent">
                    <X />
                  </button>
                </div>
              );
            })}
            <button className="btn btn-sm btn-ghost" style={{ alignSelf: 'flex-start' }} onClick={() => setAgents((l) => [...l, { address: '', weight: 1 }])}>
              <Plus /> Agent
            </button>
          </div>
        </Field>
        {checks && (
          <div className="card">
            {checks.map((c) => (
              <div key={c.address} className="row-sm small" style={{ padding: '4px 8px' }}>
                <span className="mono">{c.address}</span>
                <span className="grow" />
                {c.info ? (
                  <span className={c.info.busy ? 'text-warning' : 'subtle'}>
                    {c.info.busy ? `busy (${c.info.runId ?? 'a run'})` : 'ready'} · {c.info.machine.os}, {c.info.machine.cpus} CPUs · {c.info.version}
                  </span>
                ) : (
                  <span className="text-danger truncate" title={c.error}>
                    {c.error}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
        <div className="grid-2">
          <Field label="Agent token" hint={stored ? 'Leave empty to use the token stored in the OS keychain.' : 'The DEVALL_AGENT_TOKEN the agents run with.'}>
            <input className="input mono" type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder={stored ? '•••••• (stored in the keychain)' : 'token'} autoComplete="off" />
          </Field>
          <div className="col-sm" style={{ justifyContent: 'flex-end' }}>
            <Checkbox checked={remember} onChange={setRemember} label="Remember it in the OS keychain" />
            {stored && (
              <button
                className="btn btn-sm btn-ghost"
                style={{ alignSelf: 'flex-start' }}
                onClick={() =>
                  void loadApi
                    .agentsToken('')
                    .then(setStored)
                    .catch(() => {})
                }
              >
                Forget the stored token
              </button>
            )}
          </div>
        </div>
        {secrets.length > 0 && (
          <Callout kind="warning">
            <div className="col-sm">
              <div className="row-sm strong">
                <ShieldAlert size={14} /> {secrets.length === 1 ? 'A request uses' : `${secrets.length} requests use`} secrets: {secrets.join(', ')}
              </div>
              <div className="small">
                Agents need the resolved values to send them, so they would travel to every agent (over plain HTTP unless the agents use TLS). Without your consent the run is refused.
              </div>
              <Checkbox checked={allowSecrets} onChange={setAllowSecrets} label="Send resolved secrets to agents (I trust these agents and the network to them)" />
            </div>
          </Callout>
        )}
        <div className="row-sm subtle xs">
          New token: <code className="code-inline">devall load agent token</code>
          <CopyButton text="devall load agent token" title="Copy command" />
        </div>
      </div>
    </Modal>
  );
}
