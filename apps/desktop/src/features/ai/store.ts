import { create } from 'zustand';
import { aiApi, isLocalProvider, MANAGED_PROVIDER_ID, type ProviderConfig, type ProvidersView } from '@/lib/api/ai';
import { errorMessage } from '@/lib/ipc';

/** Providers known to the AI page and the assist buttons (`ai.json` + the running sidecar). */
interface AiStore {
  view: ProvidersView | null;
  error?: string;
  /** Provider chosen for this session (undefined = the configured default). */
  selected?: string;
  loading: boolean;
  load(): Promise<void>;
  select(id: string | undefined): void;
}

export const useAi = create<AiStore>((set) => ({
  view: null,
  loading: false,
  selected: undefined,
  async load() {
    set({ loading: true });
    try {
      set({ view: await aiApi.providers(), error: undefined });
    } catch (e) {
      set({ error: errorMessage(e) });
    } finally {
      set({ loading: false });
    }
  },
  select(id) {
    set({ selected: id });
  },
}));

/** All usable providers, the managed sidecar first when running. */
export function providerList(view: ProvidersView | null): ProviderConfig[] {
  if (!view) return [];
  return [...(view.managed ? [view.managed] : []), ...view.config.providers];
}

/** The provider a call will use (selected → default → first → sidecar). */
export function effectiveProvider(view: ProvidersView | null, selected?: string): ProviderConfig | undefined {
  const all = providerList(view);
  const want = selected ?? view?.config.defaultProvider;
  return all.find((p) => p.id === want) ?? view?.config.providers[0] ?? (view?.managed ?? undefined);
}

export const providerLabel = (p: ProviderConfig) => (p.id === MANAGED_PROVIDER_ID ? `Managed · ${p.model}` : `${p.name} · ${p.model}`);
export { isLocalProvider };

export function newCallId() {
  return `ai-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}
