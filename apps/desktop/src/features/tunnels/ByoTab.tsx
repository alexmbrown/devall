import { useEffect, useMemo, useState } from 'react';
import clsx from 'clsx';
import { Globe, Plug, RefreshCw, Server, Webhook } from 'lucide-react';
import { Callout, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { mockApi, type MockStatus } from '@/lib/api/mock';
import { webhooksApi } from '@/lib/api/webhooks';
import { useWorkspace } from '@/store/workspace';
import { portChoices, sortTunnels, validPort, type PortChoice } from './byo';
import { ToolCard, TunnelStateView } from './ByoComponents';
import { useByoTunnels } from './byoStore';

/** Running mock servers (incl. gRPC/MQTT listeners) and the project's webhook receiver. */
function useAppPorts(root: string | undefined, extra: { port: number; label: string }[]) {
  const [mocks, setMocks] = useState<MockStatus[]>([]);
  const [receiver, setReceiver] = useState<number | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const m = await mockApi.list().catch(() => [] as MockStatus[]);
      const r = root ? await webhooksApi.receiverStatus(root).catch(() => null) : null;
      if (cancelled) return;
      setMocks(m);
      setReceiver(r?.running && r.info ? r.info.port : null);
    })();
    return () => {
      cancelled = true;
    };
  }, [root, tick]);
  const choices = useMemo<PortChoice[]>(() => {
    const base = portChoices(mocks, receiver);
    const seen = new Set(base.map((c) => c.port));
    return [...base, ...extra.filter((e) => !seen.has(e.port))];
  }, [mocks, receiver, extra]);
  return { choices, rescan: () => setTick((t) => t + 1), receiver };
}

/** Tunnels → "Your own tunnel (free)": expose any local port with ngrok, cloudflared or Tailscale. */
export function ByoTab({ localPorts = [] }: { localPorts?: { port: number; label: string }[] }) {
  const project = useWorkspace((s) => s.project);
  const tools = useByoTunnels((s) => s.tools);
  const detecting = useByoTunnels((s) => s.detecting);
  const tunnels = useByoTunnels((s) => s.tunnels);
  const labels = useByoTunnels((s) => s.labels);
  const { choices, rescan, receiver } = useAppPorts(project?.root, localPorts);
  const [port, setPort] = useState('');
  const [label, setLabel] = useState<string | undefined>(undefined);
  const portNum = validPort(port);
  const pick = choices.find((c) => c.port === portNum);

  useEffect(() => {
    const st = useByoTunnels.getState();
    if (!st.tools) void st.detect();
    if (!st.settings) void st.loadSettings();
    void st.refresh();
  }, []);
  // Default to the first running server.
  useEffect(() => {
    if (!port && choices[0] && !choices[0].unsupported) {
      setPort(String(choices[0].port));
      setLabel(choices[0].label);
    }
  }, [choices, port]);

  const choose = (c: PortChoice) => {
    setPort(String(c.port));
    setLabel(c.label);
  };

  const exposeWith = async (kind: 'ngrok' | 'cloudflared' | 'tailscale') => {
    if (!portNum) {
      toast.error('Enter the local port to expose');
      return;
    }
    if (pick?.unsupported) {
      toast.error(`Can't expose ${pick.label} over HTTP`, pick.unsupported);
      return;
    }
    // Exposing the webhook receiver also makes the tunnel URL its inboxes' public URL.
    const root = project && receiver === portNum ? project.root : undefined;
    try {
      await useByoTunnels.getState().start(kind, portNum, { root, label: label ?? pick?.label ?? `Port ${portNum}` });
    } catch (e) {
      toast.error('Could not start the tunnel', errorMessage(e));
    }
  };

  const running = sortTunnels(Object.values(tunnels));

  return (
    <div className="col-lg byo-tab">
      <Callout kind="success">
        Free and local: /dev/all runs <b>your</b> ngrok, cloudflared or Tailscale Funnel — no account with us. Expose a mock server, the webhook receiver or any dev server port.
        Tokens stay in your OS keychain.
      </Callout>

      <section className="panel byo-expose">
        <div className="panel-header">
          <Plug className="tn-icon" />
          <h3 className="grow">Expose a local port</h3>
          <button className="icon-btn sm" onClick={rescan} title="Look for running servers again" aria-label="Rescan running servers">
            <RefreshCw />
          </button>
        </div>
        <div className="panel-body col">
          <div className="row wrap byo-expose-row">
            <label className="row-sm small">
              Port
              <input
                className={clsx('input sm mono byo-port', port && !portNum && 'invalid')}
                value={port}
                inputMode="numeric"
                placeholder="3000"
                onChange={(e) => {
                  setPort(e.target.value);
                  setLabel(undefined);
                }}
                aria-label="Local port"
              />
            </label>
            <div className="row-sm wrap byo-picks">
              {choices.map((c) => (
                <button
                  key={c.port}
                  className={clsx('tag byo-pick', portNum === c.port && 'on', c.unsupported && 'disabled')}
                  onClick={() => choose(c)}
                  title={c.unsupported ?? c.note ?? `Expose 127.0.0.1:${c.port}`}
                >
                  {c.label === 'Webhook receiver' ? <Webhook /> : <Server />}
                  {c.label} <span className="mono subtle">:{c.port}</span>
                </button>
              ))}
              {!choices.length && <span className="xs subtle">No mock server or receiver running — type any dev server port (e.g. 3000, 5173, 8080).</span>}
            </div>
          </div>
          {pick?.note && <div className="xs subtle">{pick.note}</div>}
          {pick?.unsupported && <Callout kind="warning">{pick.unsupported}</Callout>}
          <span className="xs subtle">Pick a tool below — each card starts a tunnel to 127.0.0.1:{portNum ?? '…'}.</span>
        </div>
      </section>

      <section className="byo-tools">
        <div className="row byo-tools-head">
          <Globe size={14} className="subtle" />
          <h3 className="grow">Your tunnel tools</h3>
          <button className="btn btn-sm btn-ghost" onClick={() => useByoTunnels.getState().detect()} disabled={detecting} title="Detect again">
            {detecting ? <Spinner /> : <RefreshCw />} Detect
          </button>
        </div>
        {!tools ? (
          <div className="row small subtle">
            <Spinner /> Looking for ngrok, cloudflared and tailscale…
          </div>
        ) : (
          <div className="byo-tool-grid">
            {tools.map((t) => (
              <ToolCard
                key={t.kind}
                tool={t}
                running={Object.values(tunnels).filter((s) => s.kind === t.kind)}
                labels={labels}
                exposeLabel={portNum ? `Expose :${portNum}` : 'Expose'}
                exposeTitle={portNum ? `Start ${t.label} for 127.0.0.1:${portNum}` : 'Enter a port above'}
                disabled={!portNum || !!pick?.unsupported}
                onExpose={() => void exposeWith(t.kind)}
              />
            ))}
          </div>
        )}
      </section>

      <section className="panel">
        <div className="panel-header">
          <Globe className="tn-icon" />
          <h3 className="grow">Running tunnels</h3>
          <span className="xs subtle">{running.filter((s) => s.state === 'online').length} online</span>
        </div>
        <div className="panel-body col-sm">
          {running.length === 0 ? (
            <span className="small subtle">None yet. They keep running while you use other pages and stop when the app quits.</span>
          ) : (
            running.map((s) => (
              <div key={s.id} className="byo-running">
                <span className="badge outline byo-running-kind">{s.kind}</span>
                <TunnelStateView s={s} label={labels[s.id]} onStop={() => useByoTunnels.getState().stop(s.id)} />
              </div>
            ))
          )}
          <span className="xs subtle">
            Also from a terminal: <code className="code-inline">devall tunnel ngrok --port 8080</code> (or <code className="code-inline">cloudflared</code>,{' '}
            <code className="code-inline">tailscale</code>) — same settings and keychain.
          </span>
        </div>
      </section>
    </div>
  );
}
