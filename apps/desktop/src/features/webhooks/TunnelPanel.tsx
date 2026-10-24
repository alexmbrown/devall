import { useEffect } from 'react';
import { ArrowRight, Globe, RefreshCw } from 'lucide-react';
import { Spinner } from '@/components/ui/misc';
import type { TunnelKind } from '@/lib/api/webhooks';
import { useNav } from '@/store/nav';
import { ToolCard } from '@/features/tunnels/ByoComponents';
import { useByoTunnels } from '@/features/tunnels/byoStore';
import { useWebhooks } from './store';

/** Detected tunnel tools (shared with Tunnels → Your own tunnel), one card each, exposing the receiver. */
export function TunnelPanel({ root, receiverPort, onNeedReceiver }: { root: string; receiverPort?: number; onNeedReceiver(): Promise<void> }) {
  const tools = useByoTunnels((s) => s.tools);
  const detecting = useByoTunnels((s) => s.detecting);
  const detect = useByoTunnels((s) => s.detect);
  const tunnels = useByoTunnels((s) => s.tunnels);
  const labels = useByoTunnels((s) => s.labels);
  const go = useNav((s) => s.go);

  useEffect(() => {
    if (!useByoTunnels.getState().tools) void detect();
    if (!useByoTunnels.getState().settings) void useByoTunnels.getState().loadSettings();
  }, [detect]);

  const expose = async (kind: TunnelKind) => {
    let port = receiverPort;
    if (!port) {
      await onNeedReceiver();
      port = useWebhooks.getState().receivers[root]?.info?.port;
    }
    if (port) await useByoTunnels.getState().start(kind, port, { root, label: 'Webhook receiver' });
  };

  return (
    <section className="wh-tunnels" aria-label="Tunnels">
      <div className="row wh-tunnels-head">
        <Globe size={14} className="subtle" />
        <span className="strong small">Expose the receiver with your own tunnel</span>
        <span className="subtle xs grow">Free: runs your installed ngrok, cloudflared or Tailscale — no account with us. A hosted URL that keeps events while this computer sleeps is available separately.</span>
        <button className="btn btn-sm btn-ghost" onClick={() => go('tunnels', 'byo')} title="All your tunnels, any port, settings">
          Tunnels <ArrowRight />
        </button>
        <button className="btn btn-sm btn-ghost" onClick={() => detect()} disabled={detecting} title="Detect again">
          {detecting ? <Spinner /> : <RefreshCw />} Detect
        </button>
      </div>
      {!tools ? (
        <div className="row small subtle">
          <Spinner /> Looking for ngrok, cloudflared and tailscale…
        </div>
      ) : (
        <div className="wh-tool-grid">
          {tools.map((t) => (
            <ToolCard
              key={t.kind}
              tool={t}
              running={Object.values(tunnels).filter((s) => s.kind === t.kind)}
              labels={labels}
              exposeLabel="Expose receiver"
              exposeTitle={receiverPort ? `Expose 127.0.0.1:${receiverPort}` : 'Starts the receiver first'}
              onExpose={() => void expose(t.kind)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
