import { create } from 'zustand';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { webhooksApi, type ProviderInfo, type ProviderKind, type ReceiverStatus } from '@/lib/api/webhooks';

/** Event-list filter value for "all events" (inbox ids are file stems, so this can't collide). */
export const ALL = '*';

/**
 * Module state that must survive navigating away from the page: receiver status per project and
 * static provider help. Bring-your-own tunnels live in `features/tunnels/byoStore.ts`.
 */
interface WebhooksState {
  providers: ProviderInfo[];
  receivers: Record<string, ReceiverStatus>;

  loadProviders(): Promise<void>;
  provider(kind: ProviderKind | undefined): ProviderInfo | undefined;
  refreshReceiver(root: string): Promise<ReceiverStatus | undefined>;
  setReceiver(root: string, s: ReceiverStatus): void;
}

export const useWebhooks = create<WebhooksState>((set, get) => ({
  providers: [],
  receivers: {},

  async loadProviders() {
    if (get().providers.length) return;
    try {
      set({ providers: await webhooksApi.providers() });
    } catch {
      /* commands not registered yet */
    }
  },

  provider(kind) {
    return get().providers.find((p) => p.kind === (kind ?? 'none'));
  },

  async refreshReceiver(root) {
    try {
      const s = await webhooksApi.receiverStatus(root);
      set((st) => ({ receivers: { ...st.receivers, [root]: s } }));
      return s;
    } catch (e) {
      toast.error('Webhooks unavailable', errorMessage(e));
    }
  },

  setReceiver(root, s) {
    set((st) => ({ receivers: { ...st.receivers, [root]: s } }));
  },
}));
