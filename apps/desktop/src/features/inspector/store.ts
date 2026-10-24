// Live capture state for the inspector. The subscription starts on first use and stays alive
// while the app runs, so traffic keeps being listed when the page is not visible.
import { create } from 'zustand';
import { errorMessage } from '@/lib/ipc';
import { toast } from '@/components/ui/toast';
import { proxyApi, type ExchangeFilter, type ExchangeId, type ExchangeSummary, type PausedExchange, type ProxyEvent, type ProxyStatus, type Rule } from '@/lib/api/proxy';
import { matches } from './filter';

interface InspectorState {
  ready: boolean;
  status: ProxyStatus | null;
  /** Mutated in place; `version` changes on every update. */
  items: Map<ExchangeId, ExchangeSummary>;
  /** Ascending ids (capture order). */
  order: ExchangeId[];
  paused: Record<ExchangeId, PausedExchange>;
  version: number;
  rules: Rule[];
  filter: ExchangeFilter;
  selected: ExchangeId[];
  /** The exchange shown in the detail pane (last clicked / keyboard focus). */
  focus: ExchangeId | null;
  anchor: ExchangeId | null;

  init(): Promise<void>;
  /** Re-subscribes after the engine was rebuilt (CA regenerated). */
  restart(): Promise<void>;
  reload(): Promise<void>;
  refreshStatus(): Promise<void>;
  setFilter(f: Partial<ExchangeFilter>): void;
  select(id: ExchangeId, mode?: 'replace' | 'toggle' | 'range'): void;
  selectMany(ids: ExchangeId[]): void;
  clearSelection(): void;
  setRules(rules: Rule[]): void;
}

let initPromise: Promise<void> | null = null;

export const useInspector = create<InspectorState>((set, get) => {
  const apply = (events: ProxyEvent[]) => {
    const s = get();
    let paused = s.paused;
    let order = s.order;
    let selected = s.selected;
    let focus = s.focus;
    let statusChanged = false;
    for (const ev of events) {
      switch (ev.type) {
        case 'exchange': {
          const x = ev.exchange;
          if (!s.items.has(x.id)) {
            if (order === s.order) order = order.slice();
            // Ids are increasing; imported/out-of-order inserts are rare, keep sorted cheaply.
            if (order.length && order[order.length - 1] > x.id) {
              const i = order.findIndex((o) => o > x.id);
              order.splice(i, 0, x.id);
            } else order.push(x.id);
          }
          s.items.set(x.id, x);
          if (!x.paused && paused[x.id]) {
            paused = { ...paused };
            delete paused[x.id];
          }
          break;
        }
        case 'paused':
          paused = { ...paused, [ev.paused.id]: ev.paused };
          break;
        case 'resumed':
          if (paused[ev.id]) {
            paused = { ...paused };
            delete paused[ev.id];
          }
          break;
        case 'removed': {
          const gone = new Set(ev.ids);
          for (const id of ev.ids) s.items.delete(id);
          order = order.filter((id) => !gone.has(id));
          selected = selected.filter((id) => !gone.has(id));
          if (focus != null && gone.has(focus)) focus = null;
          break;
        }
        case 'cleared':
          s.items.clear();
          order = [];
          selected = [];
          focus = null;
          paused = {};
          break;
        case 'status':
          statusChanged = true;
          if (ev.error) toast.error('Proxy error', ev.error);
          break;
        case 'resync':
          void get().reload();
          break;
      }
    }
    set({ order, paused, selected, focus, version: s.version + 1 });
    if (statusChanged) void get().refreshStatus();
  };

  return {
    ready: false,
    status: null,
    items: new Map(),
    order: [],
    paused: {},
    version: 0,
    rules: [],
    filter: { hideStatic: false },
    selected: [],
    focus: null,
    anchor: null,

    init() {
      if (!initPromise) {
        initPromise = (async () => {
          try {
            await proxyApi.subscribe(apply);
            await get().reload();
            const rules = await proxyApi.rules();
            set({ rules, ready: true });
          } catch (e) {
            initPromise = null;
            set({ ready: true });
            toast.error('Traffic inspector unavailable', errorMessage(e));
          }
        })();
      }
      return initPromise;
    },

    restart() {
      initPromise = null;
      return get().init();
    },

    async reload() {
      const [list, paused, status] = await Promise.all([proxyApi.list(), proxyApi.paused(), proxyApi.status()]);
      const items = new Map(list.map((x) => [x.id, x]));
      const ids = new Set(items.keys());
      set((s) => ({
        items,
        order: list.map((x) => x.id),
        paused: Object.fromEntries(paused.map((p) => [p.id, p])),
        status,
        selected: s.selected.filter((id) => ids.has(id)),
        focus: s.focus != null && ids.has(s.focus) ? s.focus : null,
        version: s.version + 1,
      }));
    },

    async refreshStatus() {
      try {
        set({ status: await proxyApi.status() });
      } catch {
        /* engine unavailable */
      }
    },

    setFilter(f) {
      set((s) => ({ filter: { ...s.filter, ...f } }));
    },

    select(id, mode = 'replace') {
      const s = get();
      if (mode === 'toggle') {
        const on = s.selected.includes(id);
        set({ selected: on ? s.selected.filter((x) => x !== id) : [...s.selected, id], focus: id, anchor: id });
      } else if (mode === 'range' && s.anchor != null) {
        const visible = visibleIds(s);
        const a = visible.indexOf(s.anchor);
        const b = visible.indexOf(id);
        if (a < 0 || b < 0) return set({ selected: [id], focus: id, anchor: id });
        const [lo, hi] = a < b ? [a, b] : [b, a];
        set({ selected: visible.slice(lo, hi + 1), focus: id });
      } else set({ selected: [id], focus: id, anchor: id });
    },

    selectMany(ids) {
      set({ selected: ids, focus: ids[ids.length - 1] ?? null });
    },

    clearSelection() {
      set({ selected: [], anchor: null });
    },

    setRules(rules) {
      set({ rules });
    },
  };
});

/** Ids passing the current filter, in capture order. */
export function visibleIds(s: Pick<InspectorState, 'order' | 'items' | 'filter'>): ExchangeId[] {
  const out: ExchangeId[] = [];
  for (const id of s.order) {
    const x = s.items.get(id);
    if (x && matches(s.filter, x)) out.push(id);
  }
  return out;
}
