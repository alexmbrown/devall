import { create } from 'zustand';
import type { AccountInfo, InboxDelivery, InboxState, InboxUpdate, RelaySettings, RelayStatus, RequestLog, TunnelEvent, TunnelStatus } from '@/lib/api/relay';

const MAX_LOG = 300;

/**
 * Tunnel page state. Lives outside the component so the live request log and inbox deliveries
 * keep flowing (Tauri channels) while the user is on another page.
 */
interface TunnelsState {
  status: RelayStatus | null;
  settings: RelaySettings | null;
  account: AccountInfo | null;
  accountError: string | null;
  tunnel: TunnelStatus | null;
  log: RequestLog[];
  inboxes: Record<string, InboxState>;
  deliveries: Record<string, InboxDelivery[]>;
  set(patch: Partial<Pick<TunnelsState, 'status' | 'settings' | 'account' | 'accountError' | 'tunnel'>>): void;
  onTunnelEvent(e: TunnelEvent): void;
  onInboxUpdate(root: string, e: InboxUpdate): void;
  setInbox(root: string, s: InboxState | null): void;
  clearLog(): void;
}

export const useTunnels = create<TunnelsState>((set) => ({
  status: null,
  settings: null,
  account: null,
  accountError: null,
  tunnel: null,
  log: [],
  inboxes: {},
  deliveries: {},
  set: (patch) => set(patch),
  onTunnelEvent: (e) => {
    if (e.type === 'status') {
      const { type: _t, ...status } = e;
      void _t;
      set({ tunnel: status });
    } else {
      const { type: _t, ...entry } = e;
      void _t;
      set((s) => ({ log: [entry, ...s.log].slice(0, MAX_LOG) }));
    }
  },
  onInboxUpdate: (root, e) => {
    if (e.type === 'status') {
      const { type: _t, ...state } = e;
      void _t;
      set((s) => ({ inboxes: { ...s.inboxes, [root]: state } }));
    } else {
      const { type: _t, ...d } = e;
      void _t;
      set((s) => ({ deliveries: { ...s.deliveries, [root]: [d, ...(s.deliveries[root] ?? [])].slice(0, 100) } }));
    }
  },
  setInbox: (root, state) =>
    set((s) => {
      const inboxes = { ...s.inboxes };
      if (state) inboxes[root] = state;
      else delete inboxes[root];
      return { inboxes };
    }),
  clearLog: () => set({ log: [] }),
}));
