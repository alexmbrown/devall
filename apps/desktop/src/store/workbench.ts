import { create } from 'zustand';
import { uid } from '@/lib/id';
import { isSingleton, type Section } from '@/lib/sections';
import * as M from '@/lib/workbench/model';
import type { Layout, OpenOptions, Side, View, ViewSpec } from '@/lib/workbench/model';

export type { Layout, OpenOptions, Side, View, ViewSpec } from '@/lib/workbench/model';

/**
 * Asked before a view closes (e.g. "save changes?" for the last view of a dirty request).
 * Resolve false to keep the view open.
 */
export type CloseGuard = (view: View, layout: Layout) => Promise<boolean>;

const guards = new Map<Section, CloseGuard>();

/** Registers the close guard of a section (one per section; later registrations replace earlier ones). */
export function registerCloseGuard(section: Section, guard: CloseGuard) {
  guards.set(section, guard);
}

interface WorkbenchState extends Layout {
  /** Project root this layout belongs to (its persistence key); null while no project is open. */
  scope: string | null;

  /** Opens a section from outside any view (sidebar, palette, links): focus an existing tab or open a preview. */
  open(section: Section, param?: string, opts?: OpenOptions): void;
  /** `go()` from inside view `viewId` — see `goFromView` in the layout model. */
  goFrom(viewId: string, section: Section, param: string | undefined, opts?: OpenOptions): void;
  focusView(id: string): void;
  focusGroup(id: string): void;
  focusGroupIndex(index: number): void;
  cycle(delta: number): void;
  keep(id: string): void;
  keepDoc(section: Section, param: string): void;
  pin(id: string, pinned: boolean): void;
  /** Sets the label a view's page gives it (its tab title); undefined clears it. */
  label(id: string, label: string | undefined): void;
  /** Closes a view after its section's close guard agreed. Resolves false when the user kept it. */
  close(id: string, opts?: { force?: boolean }): Promise<boolean>;
  closeMany(ids: string[]): Promise<void>;
  closeGroup(groupId: string): Promise<void>;
  move(viewId: string, groupId: string, index?: number): void;
  moveToSplit(viewId: string, groupId: string, side: Side): void;
  /** Opens a section in a new group beside `groupId` (a sidebar item dropped on a group edge). */
  openInSplit(spec: ViewSpec, groupId: string, side: Side): void;
  /** Splits the view's group: duplicates the view into the new group, or moves it for single-instance sections. */
  split(viewId: string | null, side: Side): void;
  /** Tab menu "Open to the side": moves the view into the neighbouring group (splitting when needed). */
  toSide(viewId: string): void;
  resize(splitId: string, sizes: number[]): void;
  /** Re-points document views after a rename/move (no guards). */
  repathDocs(section: Section, from: string, to: string): void;
  /** Closes the views of documents at or under `paths` without asking (they were deleted or moved away). */
  dropDocs(section: Section, paths: string[]): void;
  /** Switches to the layout saved for `scope` (or `fallback()` / the default when there is none). */
  load(scope: string | null, fallback?: () => ViewSpec[] | null): void;
}

const KEY = (scope: string | null) => `devall.workbench:${scope ?? ''}`;

const layoutOf = (s: Layout): Layout => ({ tree: s.tree, views: s.views, activeGroup: s.activeGroup });

function read(scope: string | null): unknown {
  try {
    return JSON.parse(localStorage.getItem(KEY(scope)) ?? 'null');
  } catch {
    return null;
  }
}

function defaultSpecs(scope: string | null): ViewSpec[] {
  return [{ section: scope ? 'http' : 'projects' }];
}

export const useWorkbench = create<WorkbenchState>((set, get) => {
  const apply = (fn: (l: Layout) => Layout) => {
    const cur = layoutOf(get());
    const next = fn(cur);
    if (next.tree !== cur.tree || next.views !== cur.views || next.activeGroup !== cur.activeGroup) set(next);
  };
  return {
    ...M.createLayout([{ section: 'http' }], uid),
    scope: null,

    open: (section, param, opts = {}) => apply((l) => M.openSection(l, section, param, opts, uid)),
    goFrom: (viewId, section, param, opts = {}) => apply((l) => M.goFromView(l, viewId, section, param, opts, uid)),
    focusView: (id) => apply((l) => M.focusView(l, id)),
    focusGroup: (id) => apply((l) => M.focusGroup(l, id)),
    focusGroupIndex: (i) => apply((l) => M.focusGroupIndex(l, i)),
    cycle: (d) => apply((l) => M.cycleTab(l, d)),
    keep: (id) => apply((l) => M.keepView(l, id)),
    keepDoc: (section, param) => apply((l) => M.keepDoc(l, section, param)),
    pin: (id, pinned) => apply((l) => M.pinView(l, id, pinned)),
    label: (id, label) => apply((l) => M.labelView(l, id, label?.trim().slice(0, M.MAX_LABEL) || undefined)),

    async close(id, opts) {
      const v = get().views[id];
      if (!v) return true;
      const guard = guards.get(v.section);
      if (guard && !opts?.force && !(await guard(v, layoutOf(get())))) return false;
      apply((l) => M.closeView(l, id));
      return true;
    },
    async closeMany(ids) {
      for (const id of ids) if (!(await get().close(id))) return;
    },
    async closeGroup(groupId) {
      const g = M.getGroup(layoutOf(get()), groupId);
      if (!g) return;
      for (const id of [...g.tabs]) if (!(await get().close(id))) return;
      apply((l) => M.removeGroup(l, groupId));
    },
    move: (viewId, groupId, index) => apply((l) => M.moveView(l, viewId, groupId, index)),
    moveToSplit: (viewId, groupId, side) => apply((l) => M.moveViewToSplit(l, viewId, groupId, side, uid)),
    openInSplit: (spec, groupId, side) => apply((l) => M.openInSplit(l, spec, groupId, side, uid)),
    split(viewId, side) {
      apply((l) => {
        const g = viewId ? M.groupOfView(l, viewId) : M.activeGroup(l);
        if (!g) return l;
        const v = viewId ? l.views[viewId] : g.active ? l.views[g.active] : undefined;
        // Single-instance sections move into the new group instead of being duplicated.
        if (v && isSingleton(v.section) && g.tabs.length > 1) return M.moveViewToSplit(l, v.id, g.id, side, uid);
        const s = M.splitGroup(l, g.id, side, uid);
        if (!s.groupId || !v || isSingleton(v.section)) return s.layout;
        return M.insertView(s.layout, s.groupId, { section: v.section, param: v.param }, { preview: false }, uid).layout;
      });
    },
    toSide(viewId) {
      apply((l) => {
        const g = M.groupOfView(l, viewId);
        const v = l.views[viewId];
        if (!g || !v) return l;
        if (g.tabs.length > 1) {
          const gs = M.allGroups(l.tree);
          const i = gs.findIndex((x) => x.id === g.id);
          if (i < gs.length - 1) return M.moveView(l, viewId, gs[i + 1].id);
          return M.moveViewToSplit(l, viewId, g.id, 'right', uid);
        }
        // The group's only tab: show a second copy beside it (single-instance sections can't).
        if (isSingleton(v.section)) return l;
        return M.openSection(l, v.section, v.param, { group: g.id, toSide: true }, uid);
      });
    },
    resize: (splitId, sizes) => apply((l) => M.resizeSplit(l, splitId, sizes)),
    repathDocs: (section, from, to) => apply((l) => M.repathDocs(l, section, from, to)),
    dropDocs(section, paths) {
      apply((l) => M.docViews(l, section, paths).reduce((acc, id) => M.closeView(acc, id), l));
    },
    load(scope, fallback) {
      let l = M.restoreLayout(read(scope), uid);
      if (!l) l = M.createLayout(fallback?.() ?? defaultSpecs(scope), uid);
      set({ ...l, scope });
    },
  };
});

// Persist the layout per project (debounced).
let saveTimer: ReturnType<typeof setTimeout> | undefined;
useWorkbench.subscribe((s, prev) => {
  if (s.tree === prev.tree && s.views === prev.views && s.activeGroup === prev.activeGroup && s.scope === prev.scope) return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const cur = useWorkbench.getState();
    try {
      localStorage.setItem(KEY(cur.scope), JSON.stringify(M.serializeLayout(layoutOf(cur))));
    } catch {
      /* storage unavailable */
    }
  }, 250);
});

/** The active view of the active group. */
export function activeViewOf(s: Layout): View | undefined {
  return M.activeView(s);
}

/** The document most recently shown by a view of `section` (active group first), filtered by `pred`. */
export function recentDoc(section: Section, pred: (param: string) => boolean = () => true): string | undefined {
  const l = layoutOf(useWorkbench.getState());
  return M.findViews(l, (v) => v.section === section && !!v.param && pred(v.param))[0]?.param;
}
