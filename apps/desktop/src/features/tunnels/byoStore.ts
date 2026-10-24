import { create } from 'zustand';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type ToolInfo, type TunnelKind, type TunnelSettings, type TunnelSettingsView, type TunnelStatus } from '@/lib/api/webhooks';
import { useWebhooks } from '@/features/webhooks/store';

/**
 * Bring-your-own tunnels (ngrok, cloudflared, Tailscale Funnel), shared by the Tunnels page and the
 * Webhooks page. Lives outside components so running tunnels keep reporting (their status
 * channels write here) while the user is elsewhere.
 */
interface ByoState {
  tools: ToolInfo[] | null;
  detecting: boolean;
  tunnels: Record<string, TunnelStatus>;
  /** Tunnel id → project root whose webhook inboxes it exposes. */
  roots: Record<string, string>;
  /** Tunnel id → what it exposes ("Mock server", "Webhook receiver", "Port 3000"). */
  labels: Record<string, string>;
  settings: TunnelSettingsView | null;

  detect(): Promise<void>;
  refresh(): Promise<void>;
  loadSettings(): Promise<TunnelSettingsView | null>;
  saveSettings(s: TunnelSettings): Promise<boolean>;
  start(kind: TunnelKind, port: number, opts?: { root?: string; label?: string }): Promise<TunnelStatus | undefined>;
  stop(id: string): Promise<void>;
}

export const useByoTunnels = create<ByoState>((set, get) => ({
  tools: null,
  detecting: false,
  tunnels: {},
  roots: {},
  labels: {},
  settings: null,

  async detect() {
    set({ detecting: true });
    try {
      set({ tools: await webhooksApi.tunnelDetect() });
    } catch (e) {
      toast.error('Could not detect tunnel tools', errorMessage(e));
    } finally {
      set({ detecting: false });
    }
  },

  async refresh() {
    try {
      const list = await webhooksApi.tunnelStatus();
      set({ tunnels: Object.fromEntries(list.map((t) => [t.id, t])) });
    } catch {
      /* module not registered */
    }
  },

  async loadSettings() {
    try {
      const v = await webhooksApi.tunnelSettings();
      set({ settings: v });
      return v;
    } catch (e) {
      toast.error('Could not read tunnel settings', errorMessage(e));
      return null;
    }
  },

  async saveSettings(s) {
    try {
      await webhooksApi.tunnelSettingsSave(s);
      await get().loadSettings();
      return true;
    } catch (e) {
      toast.error('Settings not saved', errorMessage(e));
      return false;
    }
  },

  async start(kind, port, opts) {
    const root = opts?.root;
    try {
      const first = await webhooksApi.tunnelStart(kind, port, root, (s) => {
        set((st) => ({ tunnels: { ...st.tunnels, [s.id]: s } }));
        if (root && s.publicUrl && s.state === 'online') {
          // The receiver's public base changed; refresh it so inbox URLs update.
          void useWebhooks.getState().refreshReceiver(root);
        }
      });
      set((st) => ({
        tunnels: { ...st.tunnels, [first.id]: { ...first, ...st.tunnels[first.id] } },
        roots: root ? { ...st.roots, [first.id]: root } : st.roots,
        labels: { ...st.labels, [first.id]: opts?.label ?? `Port ${port}` },
      }));
      return first;
    } catch (e) {
      toast.error(`Could not start ${kind}`, errorMessage(e));
    }
  },

  async stop(id) {
    try {
      await webhooksApi.tunnelStop(id);
    } catch (e) {
      toast.error('Could not stop the tunnel', errorMessage(e));
    }
    const root = get().roots[id];
    set((st) => {
      const tunnels = { ...st.tunnels };
      delete tunnels[id];
      const roots = { ...st.roots };
      delete roots[id];
      return { tunnels, roots };
    });
    if (root) void useWebhooks.getState().refreshReceiver(root);
  },
}));

export const isLive = (s: TunnelStatus) => s.state === 'online' || s.state === 'starting';
