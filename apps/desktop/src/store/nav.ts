import { createContext, useCallback, useContext, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { Section } from '@/lib/sections';
import type { OpenOptions } from '@/lib/workbench/model';
import { activeViewOf, useWorkbench } from './workbench';

export type { Section } from '@/lib/sections';

/** Options for `go()`: `preview: false` opens a permanent tab, `newTab` always opens another tab, `toSide` the neighbouring group. */
export type GoOptions = Pick<OpenOptions, 'preview' | 'newTab' | 'toSide'>;

export interface NavState {
  section: Section;
  /** Free-form sub-route (e.g. a utility op id or environment id) — or a document path for `http`/`notes`. */
  param?: string;
  go(section: Section, param?: string, opts?: GoOptions): void;
}

/**
 * The workbench view a component renders in (provided by each tab). `useNav` reads it, so pages
 * keep calling `useNav((s) => s.param)` / `go()` and get *their* view's route.
 */
export const ViewContext = createContext<string | null>(null);

const goCache = new Map<string, NavState['go']>();

function goFor(viewId: string): NavState['go'] {
  let go = goCache.get(viewId);
  if (!go) {
    go = (section, param, opts) => useWorkbench.getState().goFrom(viewId, section, param, opts ?? {});
    goCache.set(viewId, go);
  }
  return go;
}

/** Outside any view: act on the active view of the active group (focus or open a tab). */
const globalGo: NavState['go'] = (section, param, opts) => useWorkbench.getState().open(section, param, opts ?? {});

function useNavImpl<T>(selector?: (s: NavState) => T): T | NavState {
  const viewId = useContext(ViewContext);
  const section = useWorkbench((s) => (viewId && s.views[viewId] ? s.views[viewId].section : (activeViewOf(s)?.section ?? 'http')));
  const param = useWorkbench((s) => (viewId && s.views[viewId] ? s.views[viewId].param : activeViewOf(s)?.param));
  const go = viewId ? goFor(viewId) : globalGo;
  const state = useMemo<NavState>(() => ({ section, param, go }), [section, param, go]);
  return selector ? selector(state) : state;
}

interface UseNav {
  (): NavState;
  <T>(selector: (s: NavState) => T): T;
  /** Navigation of the active view in the active group — for code outside React or outside any view. */
  getState(): NavState;
}

/**
 * Navigation facade over the workbench. Inside a view (any component under a tab), `section`/`param`
 * are that view's and `go()` navigates it: same section → the view itself (document sections open
 * other documents as tabs, reusing a preview/empty view); another section → focus or open a tab for
 * it next to this one. Outside views (top bar, palette, global shortcuts, `getState()`), it acts on
 * the active view: `go()` focuses an existing tab for the section or opens a preview tab.
 */
export const useNav: UseNav = Object.assign(useNavImpl as UseNav, {
  getState(): NavState {
    const v = activeViewOf(useWorkbench.getState());
    return { section: v?.section ?? 'http', param: v?.param, go: globalGo };
  },
});

/** Id of the view this component renders in (null outside the workbench). */
export function useViewId(): string | null {
  return useContext(ViewContext);
}

/** Returns a function that turns this component's view from a preview tab into a normal one (call on first edit). */
export function useKeepView(): () => void {
  const viewId = useContext(ViewContext);
  return useCallback(() => {
    if (viewId && useWorkbench.getState().views[viewId]?.preview) useWorkbench.getState().keep(viewId);
  }, [viewId]);
}

/**
 * Names what this component's view shows — the selected utility, environment, log source… — so its
 * tab reads "Regex escape" instead of "Utilities". Pass a string that reads on its own (the tab
 * keeps the section icon; the tooltip adds the section name), `null` for the section's landing
 * page (the tab shows the section name), or `undefined` while it is still loading (keeps the label
 * the view already has, e.g. the one restored with the layout).
 */
export function useViewLabel(label: string | null | undefined): void {
  const viewId = useContext(ViewContext);
  useLayoutEffect(() => {
    if (!viewId || label === undefined) return;
    const next = label?.trim() || undefined;
    if (useWorkbench.getState().views[viewId]?.label !== next) useWorkbench.getState().label(viewId, next);
  }, [viewId, label]);
}

/** True while this component's view is the active view of the active group (always true outside views). */
export function useIsActiveView(): boolean {
  const viewId = useContext(ViewContext);
  return useWorkbench((s) => !viewId || activeViewOf(s)?.id === viewId);
}

/**
 * A ref that is true while this component's view is active — for window-level key handlers, so two
 * visible views (a split) don't both react to Ctrl+S / Ctrl+Enter.
 */
export function useActiveViewRef(): { readonly current: boolean } {
  const active = useIsActiveView();
  const ref = useRef(active);
  ref.current = active;
  return ref;
}

const viewState = new Map<string, unknown>();

/**
 * useState that survives tab switches (inactive tabs unmount) for the lifetime of the view, e.g. the
 * selected sub-tab of an editor. Outside views it behaves like plain useState.
 */
export function useViewState<T>(key: string, initial: T | (() => T)): [T, (v: T | ((p: T) => T)) => void] {
  const viewId = useContext(ViewContext);
  const k = viewId ? `${viewId}|${key}` : null;
  const [value, setValue] = useState<T>(() => (k && viewState.has(k) ? (viewState.get(k) as T) : typeof initial === 'function' ? (initial as () => T)() : initial));
  const set = useCallback(
    (v: T | ((p: T) => T)) =>
      setValue((prev) => {
        const next = typeof v === 'function' ? (v as (p: T) => T)(prev) : v;
        if (k) viewState.set(k, next);
        return next;
      }),
    [k],
  );
  return [value, set];
}

// Forget per-view state and cached navigators of closed views.
useWorkbench.subscribe((s, prev) => {
  if (s.views === prev.views) return;
  for (const id of Object.keys(prev.views)) {
    if (s.views[id]) continue;
    goCache.delete(id);
    for (const k of [...viewState.keys()]) if (k.startsWith(`${id}|`)) viewState.delete(k);
  }
});
