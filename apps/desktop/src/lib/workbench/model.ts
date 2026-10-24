/**
 * Workbench layout model: editor groups arranged in a split tree, each with its own tab strip.
 * A tab is a *view* — one instance of a section (`http`, `mocks`, …) with an optional `param`.
 *
 * Everything here is pure (layout in, layout out) so it is unit-tested without React; the zustand
 * store in `store/workbench.ts` wraps it and adds persistence and close guards.
 */
import { isDocSection, isSection, isSingleton, type Section } from '@/lib/sections';

export interface View {
  id: string;
  section: Section;
  /** Sub-route (utility op, environment id, …) — or, for document sections, the document path. */
  param?: string;
  /**
   * What the view currently shows, set by its page (the selected utility, environment, log source…)
   * and used as the tab title. Persisted, so inactive (unmounted) and restored tabs keep it.
   */
  label?: string;
  /** Pinned tabs sit at the start of the strip and are not closed by Ctrl+W, middle-click or "Close others". */
  pinned: boolean;
  /** Preview tab (italic): replaced by the next preview opened in the same group. */
  preview: boolean;
}

export interface GroupNode {
  kind: 'group';
  id: string;
  /** View ids in strip order. */
  tabs: string[];
  active: string | null;
  /** Most recently focused first. */
  mru: string[];
}

export type SplitDir = 'row' | 'col';

export interface SplitNode {
  kind: 'split';
  id: string;
  /** `row` = children side by side, `col` = stacked. */
  dir: SplitDir;
  children: LayoutNode[];
  /** Fractions of the split's size, one per child, summing to 1. */
  sizes: number[];
}

export type LayoutNode = GroupNode | SplitNode;

export interface Layout {
  tree: LayoutNode;
  views: Record<string, View>;
  activeGroup: string;
}

export type IdGen = () => string;
export type Side = 'left' | 'right' | 'up' | 'down';

export const MAX_GROUPS = 4;
const MIN_SIZE = 0.1;
/** Longest view label kept (labels are tab titles, not content). */
export const MAX_LABEL = 120;

export interface ViewSpec {
  section: Section;
  param?: string;
}

export interface OpenOptions {
  /** Target group (default: the active group). */
  group?: string;
  /** Open as a preview tab (default true); `false` also turns an existing preview tab permanent. */
  preview?: boolean;
  /** Always open a new permanent tab (singletons still focus their one view). */
  newTab?: boolean;
  /** Open in the neighbouring group, splitting to the right when there is only one. */
  toSide?: boolean;
}

// ---------- queries ----------

export function allGroups(node: LayoutNode, out: GroupNode[] = []): GroupNode[] {
  if (node.kind === 'group') out.push(node);
  else for (const c of node.children) allGroups(c, out);
  return out;
}

export function getGroup(l: Layout, id: string): GroupNode | undefined {
  return allGroups(l.tree).find((g) => g.id === id);
}

export function groupOfView(l: Layout, viewId: string): GroupNode | undefined {
  return allGroups(l.tree).find((g) => g.tabs.includes(viewId));
}

export function activeGroup(l: Layout): GroupNode {
  const gs = allGroups(l.tree);
  return gs.find((g) => g.id === l.activeGroup) ?? gs[0];
}

export function activeView(l: Layout): View | undefined {
  const g = activeGroup(l);
  return g.active ? l.views[g.active] : undefined;
}

/** Views matching `pred`, most recently used first within each group, active group first. */
export function findViews(l: Layout, pred: (v: View) => boolean, prefer?: string): View[] {
  const gs = allGroups(l.tree);
  const first = prefer ?? l.activeGroup;
  const ordered = [...gs.filter((g) => g.id === first), ...gs.filter((g) => g.id !== first)];
  const out: View[] = [];
  for (const g of ordered) {
    const seen = new Set<string>();
    for (const id of [...g.mru, ...g.tabs]) {
      if (seen.has(id) || !g.tabs.includes(id)) continue;
      seen.add(id);
      const v = l.views[id];
      if (v && pred(v)) out.push(v);
    }
  }
  return out;
}

// ---------- tree helpers ----------

function mapNode(node: LayoutNode, id: string, fn: (n: LayoutNode) => LayoutNode): LayoutNode {
  if (node.id === id) return fn(node);
  if (node.kind === 'group') return node;
  let changed = false;
  const children = node.children.map((c) => {
    const n = mapNode(c, id, fn);
    if (n !== c) changed = true;
    return n;
  });
  return changed ? { ...node, children } : node;
}

function updateGroup(l: Layout, id: string, fn: (g: GroupNode) => GroupNode): Layout {
  return { ...l, tree: mapNode(l.tree, id, (n) => (n.kind === 'group' ? fn(n) : n)) };
}

function normalizeSizes(sizes: number[], n: number): number[] {
  const s = sizes.length === n && sizes.every((x) => Number.isFinite(x) && x > 0) ? sizes : Array.from({ length: n }, () => 1 / n);
  const sum = s.reduce((a, b) => a + b, 0);
  return s.map((x) => x / sum);
}

/** Removes the node with `id`; splits left with one child collapse into that child. */
function removeNode(node: LayoutNode, id: string): LayoutNode | null {
  if (node.id === id) return null;
  if (node.kind === 'group') return node;
  const idx = node.children.findIndex((c) => c.id === id);
  let children: LayoutNode[];
  let sizes: number[];
  if (idx >= 0) {
    children = node.children.filter((_, i) => i !== idx);
    sizes = node.sizes.filter((_, i) => i !== idx);
  } else {
    let changed = false;
    const mapped = node.children.map((c) => {
      const n = removeNode(c, id);
      if (n !== c) changed = true;
      return n;
    });
    if (!changed) return node;
    children = mapped.filter((c): c is LayoutNode => !!c);
    sizes = node.sizes.filter((_, i) => mapped[i]);
  }
  if (children.length === 0) return null;
  if (children.length === 1) return children[0];
  return flatten({ ...node, children, sizes: normalizeSizes(sizes, children.length) });
}

/** Merges child splits that run in the same direction as their parent. */
function flatten(node: SplitNode): SplitNode {
  if (!node.children.some((c) => c.kind === 'split' && c.dir === node.dir)) return node;
  const children: LayoutNode[] = [];
  const sizes: number[] = [];
  node.children.forEach((c, i) => {
    if (c.kind === 'split' && c.dir === node.dir) {
      c.children.forEach((cc, j) => {
        children.push(cc);
        sizes.push(node.sizes[i] * c.sizes[j]);
      });
    } else {
      children.push(c);
      sizes.push(node.sizes[i]);
    }
  });
  return { ...node, children, sizes: normalizeSizes(sizes, children.length) };
}

function parentOf(node: LayoutNode, id: string): SplitNode | undefined {
  if (node.kind === 'group') return undefined;
  if (node.children.some((c) => c.id === id)) return node;
  for (const c of node.children) {
    const p = parentOf(c, id);
    if (p) return p;
  }
  return undefined;
}

// ---------- tab operations ----------

const pinnedCount = (l: Layout, g: GroupNode) => g.tabs.filter((id) => l.views[id]?.pinned).length;

export function emptyGroup(id: string): GroupNode {
  return { kind: 'group', id, tabs: [], active: null, mru: [] };
}

export function createLayout(specs: ViewSpec[], gen: IdGen): Layout {
  const g = emptyGroup(gen());
  let l: Layout = { tree: g, views: {}, activeGroup: g.id };
  for (const s of specs) l = insertView(l, g.id, s, { preview: false }, gen).layout;
  if (specs.length) l = focusView(l, allGroups(l.tree)[0].tabs[0]);
  return l;
}

export function focusView(l: Layout, viewId: string): Layout {
  const g = groupOfView(l, viewId);
  if (!g) return l;
  if (g.active === viewId && g.mru[0] === viewId && l.activeGroup === g.id) return l;
  const next = updateGroup(l, g.id, (x) => ({ ...x, active: viewId, mru: [viewId, ...x.mru.filter((m) => m !== viewId)] }));
  return { ...next, activeGroup: g.id };
}

export function focusGroup(l: Layout, groupId: string): Layout {
  if (l.activeGroup === groupId || !getGroup(l, groupId)) return l;
  return { ...l, activeGroup: groupId };
}

export function focusGroupIndex(l: Layout, index: number): Layout {
  const g = allGroups(l.tree)[index];
  return g ? focusGroup(l, g.id) : l;
}

/** Inserts a new view into a group (replacing the group's preview tab when opening a preview). */
export function insertView(l: Layout, groupId: string, spec: ViewSpec, opts: { preview: boolean; index?: number; focus?: boolean }, gen: IdGen): { layout: Layout; viewId: string } {
  const g = getGroup(l, groupId);
  if (!g) return { layout: l, viewId: '' };
  const view: View = { id: gen(), section: spec.section, param: spec.param, pinned: false, preview: opts.preview };
  const views = { ...l.views, [view.id]: view };
  let tabs = [...g.tabs];
  let mru = [...g.mru];
  const old = opts.preview ? tabs.find((id) => l.views[id]?.preview) : undefined;
  if (old) {
    tabs[tabs.indexOf(old)] = view.id;
    delete views[old];
    mru = mru.filter((m) => m !== old);
  } else {
    const activeIdx = g.active ? tabs.indexOf(g.active) : -1;
    const want = opts.index ?? (activeIdx >= 0 ? activeIdx + 1 : tabs.length);
    const at = Math.max(pinnedCount(l, g), Math.min(want, tabs.length));
    tabs = [...tabs.slice(0, at), view.id, ...tabs.slice(at)];
  }
  const active = g.active === old ? view.id : g.active;
  let layout: Layout = { ...updateGroup(l, groupId, (x) => ({ ...x, tabs, mru, active: active ?? view.id })), views };
  if (opts.focus !== false) layout = focusView(layout, view.id);
  return { layout, viewId: view.id };
}

function setView(l: Layout, viewId: string, patch: Partial<View>): Layout {
  const v = l.views[viewId];
  if (!v) return l;
  const next = { ...v, ...patch };
  if (next.param === undefined) delete next.param;
  if (next.label === undefined) delete next.label;
  if (v.param === next.param && v.label === next.label && v.preview === next.preview && v.pinned === next.pinned) return l;
  return { ...l, views: { ...l.views, [viewId]: next } };
}

export function navigateView(l: Layout, viewId: string, param: string | undefined): Layout {
  return setView(l, viewId, { param });
}

/** Sets (or, with undefined, clears) the label a view's page gives it. */
export function labelView(l: Layout, viewId: string, label: string | undefined): Layout {
  return setView(l, viewId, { label });
}

/** Turns a preview tab into a normal one. */
export function keepView(l: Layout, viewId: string): Layout {
  return l.views[viewId]?.preview ? setView(l, viewId, { preview: false }) : l;
}

/** Keeps every preview tab showing a document (called on its first edit). */
export function keepDoc(l: Layout, section: Section, param: string): Layout {
  let out = l;
  for (const v of Object.values(l.views)) if (v.preview && v.section === section && v.param === param) out = keepView(out, v.id);
  return out;
}

export function pinView(l: Layout, viewId: string, pinned: boolean): Layout {
  const g = groupOfView(l, viewId);
  if (!g) return l;
  let out = setView(l, viewId, { pinned, preview: false });
  const rest = g.tabs.filter((id) => id !== viewId);
  const pins = rest.filter((id) => out.views[id]?.pinned);
  const others = rest.filter((id) => !out.views[id]?.pinned);
  // Pinning appends to the pinned block; unpinning puts the tab first among the unpinned ones.
  return updateGroup(out, g.id, (x) => ({ ...x, tabs: [...pins, viewId, ...others] }));
}

/** Removes a view; an emptied group disappears unless it is the last one. */
export function closeView(l: Layout, viewId: string): Layout {
  const g = groupOfView(l, viewId);
  if (!g) return l;
  const idx = g.tabs.indexOf(viewId);
  const tabs = g.tabs.filter((id) => id !== viewId);
  const mru = g.mru.filter((id) => id !== viewId);
  const active = g.active === viewId ? (mru.find((id) => tabs.includes(id)) ?? tabs[Math.min(idx, tabs.length - 1)] ?? null) : g.active;
  const views = { ...l.views };
  delete views[viewId];
  let out: Layout = { ...updateGroup(l, g.id, (x) => ({ ...x, tabs, mru, active })), views };
  if (!tabs.length) out = removeGroup(out, g.id);
  return out;
}

/** Removes an (empty or not) group unless it is the only one; its views are dropped. */
export function removeGroup(l: Layout, groupId: string): Layout {
  const gs = allGroups(l.tree);
  if (gs.length <= 1) return l;
  const idx = gs.findIndex((g) => g.id === groupId);
  if (idx < 0) return l;
  const tree = removeNode(l.tree, groupId);
  if (!tree) return l;
  const views = { ...l.views };
  for (const id of gs[idx].tabs) delete views[id];
  const remaining = allGroups(tree);
  const activeGroupId = l.activeGroup === groupId ? (remaining[Math.max(0, idx - 1)] ?? remaining[0]).id : l.activeGroup;
  return { tree, views, activeGroup: activeGroupId };
}

/** Ids a bulk close would remove (pinned tabs are kept). */
export function viewsToClose(l: Layout, viewId: string, which: 'others' | 'right' | 'all'): string[] {
  const g = groupOfView(l, viewId);
  if (!g) return [];
  const idx = g.tabs.indexOf(viewId);
  return g.tabs.filter((id, i) => !l.views[id]?.pinned && (which === 'all' || (which === 'others' ? id !== viewId : i > idx)));
}

/** Moves a view within its group or into another group at `index` (default: the end). */
export function moveView(l: Layout, viewId: string, targetGroupId: string, index?: number): Layout {
  const src = groupOfView(l, viewId);
  const dst = getGroup(l, targetGroupId);
  const v = l.views[viewId];
  if (!src || !dst || !v) return l;
  if (src.id === dst.id) {
    const from = src.tabs.indexOf(viewId);
    const rest = src.tabs.filter((id) => id !== viewId);
    let at = index === undefined ? rest.length : index > from ? index - 1 : index;
    const pins = rest.filter((id) => l.views[id]?.pinned).length;
    at = v.pinned ? Math.min(Math.max(0, at), pins) : Math.max(pins, Math.min(at, rest.length));
    const tabs = [...rest.slice(0, at), viewId, ...rest.slice(at)];
    if (tabs.every((id, i) => id === src.tabs[i])) return focusView(l, viewId);
    return focusView(updateGroup(l, src.id, (x) => ({ ...x, tabs })), viewId);
  }
  // Leave the source group.
  const sIdx = src.tabs.indexOf(viewId);
  const sTabs = src.tabs.filter((id) => id !== viewId);
  const sMru = src.mru.filter((id) => id !== viewId);
  const sActive = src.active === viewId ? (sMru.find((id) => sTabs.includes(id)) ?? sTabs[Math.min(sIdx, sTabs.length - 1)] ?? null) : src.active;
  let out = updateGroup(l, src.id, (x) => ({ ...x, tabs: sTabs, mru: sMru, active: sActive }));
  // A group holds at most one preview tab.
  if (v.preview && dst.tabs.some((id) => l.views[id]?.preview)) out = setView(out, viewId, { preview: false });
  const moved = out.views[viewId];
  const dTabs = [...dst.tabs];
  const pins = dTabs.filter((id) => out.views[id]?.pinned).length;
  const want = index ?? dTabs.length;
  const at = moved.pinned ? Math.min(Math.max(0, want), pins) : Math.max(pins, Math.min(want, dTabs.length));
  dTabs.splice(at, 0, viewId);
  out = updateGroup(out, dst.id, (x) => ({ ...x, tabs: dTabs }));
  out = focusView(out, viewId);
  if (!sTabs.length) out = removeGroup(out, src.id);
  return out;
}

/**
 * Splits `groupId` and returns the new (empty) group's id — or null when the layout already has
 * `MAX_GROUPS` groups.
 */
export function splitGroup(l: Layout, groupId: string, side: Side, gen: IdGen): { layout: Layout; groupId: string | null } {
  if (allGroups(l.tree).length >= MAX_GROUPS || !getGroup(l, groupId)) return { layout: l, groupId: null };
  const fresh = emptyGroup(gen());
  const dir: SplitDir = side === 'left' || side === 'right' ? 'row' : 'col';
  const before = side === 'left' || side === 'up';
  const parent = parentOf(l.tree, groupId);
  let tree: LayoutNode;
  if (parent && parent.dir === dir) {
    tree = mapNode(l.tree, parent.id, (n) => {
      const p = n as SplitNode;
      const i = p.children.findIndex((c) => c.id === groupId);
      const half = p.sizes[i] / 2;
      const children = [...p.children];
      const sizes = [...p.sizes];
      children.splice(before ? i : i + 1, 0, fresh);
      sizes.splice(i, 1, half, half);
      return { ...p, children, sizes };
    });
  } else {
    tree = mapNode(l.tree, groupId, (g) => ({ kind: 'split', id: gen(), dir, children: before ? [fresh, g] : [g, fresh], sizes: [0.5, 0.5] }));
  }
  return { layout: { ...l, tree, activeGroup: fresh.id }, groupId: fresh.id };
}

/** Moves a view into a new group on `side` of `targetGroupId` (drag onto a group's edge). */
export function moveViewToSplit(l: Layout, viewId: string, targetGroupId: string, side: Side, gen: IdGen): Layout {
  const src = groupOfView(l, viewId);
  if (!src) return l;
  // Dragging a group's only tab onto its own edge would just recreate the same layout.
  if (src.id === targetGroupId && src.tabs.length === 1) return l;
  const { layout, groupId } = splitGroup(l, targetGroupId, side, gen);
  if (!groupId) return moveView(l, viewId, targetGroupId);
  return moveView(layout, viewId, groupId);
}

export function resizeSplit(l: Layout, splitId: string, sizes: number[]): Layout {
  return {
    ...l,
    tree: mapNode(l.tree, splitId, (n) => {
      if (n.kind !== 'split' || sizes.length !== n.children.length) return n;
      const clamped = sizes.map((s) => Math.max(MIN_SIZE, s));
      return { ...n, sizes: normalizeSizes(clamped, n.children.length) };
    }),
  };
}

/** Next/previous tab of the active group (wraps). */
export function cycleTab(l: Layout, delta: number): Layout {
  const g = activeGroup(l);
  if (g.tabs.length < 2) return l;
  const i = g.active ? g.tabs.indexOf(g.active) : 0;
  const next = g.tabs[(i + delta + g.tabs.length * 4) % g.tabs.length];
  return focusView(l, next);
}

/** The group a "to the side" open lands in: the next group, else a new split on the right, else the previous one. */
function sideGroup(l: Layout, fromGroup: string, gen: IdGen): { layout: Layout; groupId: string } {
  const gs = allGroups(l.tree);
  const i = gs.findIndex((g) => g.id === fromGroup);
  if (i >= 0 && i < gs.length - 1) return { layout: l, groupId: gs[i + 1].id };
  const s = splitGroup(l, fromGroup, 'right', gen);
  if (s.groupId) return { layout: s.layout, groupId: s.groupId };
  return { layout: l, groupId: gs[Math.max(0, i - 1)].id };
}

// ---------- navigation semantics ----------

/**
 * Opens a section the way the sidebar, the command palette and code outside any view do:
 * focus an existing tab for it (a document section matches on `param`) or open a preview tab.
 */
export function openSection(l: Layout, section: Section, param: string | undefined, opts: OpenOptions, gen: IdGen): Layout {
  const groupId = opts.group && getGroup(l, opts.group) ? opts.group : activeGroup(l).id;
  const doc = isDocSection(section);
  const keep = (x: Layout, id: string) => (opts.preview === false ? keepView(x, id) : x);

  if (opts.toSide) {
    const existing = isSingleton(section) ? findViews(l, (v) => v.section === section)[0] : undefined;
    const side = sideGroup(l, groupId, gen);
    if (existing) {
      let out = existing.param !== param && param !== undefined ? navigateView(side.layout, existing.id, param) : side.layout;
      out = groupOfView(out, existing.id)?.id === side.groupId ? focusView(out, existing.id) : moveView(out, existing.id, side.groupId);
      return keepView(out, existing.id);
    }
    const sideG = getGroup(side.layout, side.groupId)!;
    const same = doc && param ? sideG.tabs.find((id) => side.layout.views[id]?.section === section && side.layout.views[id]?.param === param) : undefined;
    if (same) return keepView(focusView(side.layout, same), same);
    return insertView(side.layout, side.groupId, { section, param }, { preview: false }, gen).layout;
  }

  if (isSingleton(section)) {
    const existing = findViews(l, (v) => v.section === section, groupId)[0];
    if (existing) {
      const out = param !== undefined ? navigateView(l, existing.id, param) : l;
      return keep(focusView(out, existing.id), existing.id);
    }
    return insertView(l, groupId, { section, param }, { preview: !opts.newTab && opts.preview !== false }, gen).layout;
  }

  if (opts.newTab) return insertView(l, groupId, { section, param }, { preview: false }, gen).layout;

  if (doc && param) {
    const same = findViews(l, (v) => v.section === section && v.param === param, groupId)[0];
    if (same) return keep(focusView(l, same.id), same.id);
    // An empty landing view of the section (e.g. "Requests" with nothing open) takes the document.
    const g = getGroup(l, groupId)!;
    const cur = g.active ? l.views[g.active] : undefined;
    if (cur && cur.section === section && !cur.param) return keep(focusView(navigateView(l, cur.id, param), cur.id), cur.id);
    return insertView(l, groupId, { section, param }, { preview: opts.preview !== false }, gen).layout;
  }

  const existing = findViews(l, (v) => v.section === section, groupId)[0];
  if (existing) {
    const out = param !== undefined && !doc ? navigateView(l, existing.id, param) : l;
    return keep(focusView(out, existing.id), existing.id);
  }
  return insertView(l, groupId, { section, param }, { preview: opts.preview !== false }, gen).layout;
}

/**
 * `go()` called from inside a view. Same section: navigates that view (document sections open
 * other documents as tabs, reusing the view when it is a preview or empty). Another section:
 * behaves like {@link openSection} in the view's group, so the current tab stays where it is.
 */
export function goFromView(l: Layout, viewId: string, section: Section, param: string | undefined, opts: OpenOptions, gen: IdGen): Layout {
  const v = l.views[viewId];
  const g = v && groupOfView(l, viewId);
  if (!v || !g) return openSection(l, section, param, opts, gen);
  if (section !== v.section || opts.newTab || opts.toSide) return openSection(l, section, param, { ...opts, group: g.id }, gen);
  const keep = (x: Layout, id: string) => (opts.preview === false ? keepView(x, id) : x);
  if (isDocSection(section) && param && param !== v.param) {
    const same = findViews(l, (x) => x.section === section && x.param === param, g.id)[0];
    if (same) return keep(focusView(l, same.id), same.id);
    if (v.preview || !v.param) return keep(focusView(navigateView(l, viewId, param), viewId), viewId);
    return insertView(l, g.id, { section, param }, { preview: opts.preview !== false }, gen).layout;
  }
  return keep(focusView(navigateView(l, viewId, param), viewId), viewId);
}

// ---------- documents ----------

/** Re-points views of documents under `from` (a file or folder path) to `to`. */
export function repathDocs(l: Layout, section: Section, from: string, to: string): Layout {
  let out = l;
  for (const v of Object.values(l.views)) {
    if (v.section !== section || !v.param) continue;
    if (v.param === from) out = navigateView(out, v.id, to);
    else if (v.param.startsWith(`${from}/`)) out = navigateView(out, v.id, to + v.param.slice(from.length));
  }
  return out;
}

/** Views of documents at or under any of `paths`. */
export function docViews(l: Layout, section: Section, paths: string[]): string[] {
  const hit = (p: string) => paths.some((x) => p === x || p.startsWith(`${x}/`));
  return Object.values(l.views)
    .filter((v) => v.section === section && v.param !== undefined && hit(v.param))
    .map((v) => v.id);
}

// ---------- persistence ----------

export interface SavedLayout {
  v: 1;
  tree: LayoutNode;
  views: Record<string, View>;
  activeGroup: string;
}

export function serializeLayout(l: Layout): SavedLayout {
  return { v: 1, tree: l.tree, views: l.views, activeGroup: l.activeGroup };
}

/**
 * Validates and repairs a persisted layout (unknown sections, dangling ids, duplicate singletons,
 * bad sizes). Returns null when nothing usable is left.
 */
export function restoreLayout(raw: unknown, gen: IdGen): Layout | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Partial<SavedLayout>;
  const rawViews = r.views && typeof r.views === 'object' ? r.views : {};
  const views: Record<string, View> = {};
  const used = new Set<string>();
  const singletons = new Set<Section>();
  const groupIds = new Set<string>();

  const fixGroup = (n: Partial<GroupNode>): GroupNode | null => {
    const id = typeof n.id === 'string' && !groupIds.has(n.id) ? n.id : gen();
    groupIds.add(id);
    const tabs: string[] = [];
    let preview = false;
    for (const t of Array.isArray(n.tabs) ? n.tabs : []) {
      const v = (rawViews as Record<string, View>)[t];
      if (typeof t !== 'string' || used.has(t) || !v || !isSection(v.section)) continue;
      if (isSingleton(v.section)) {
        if (singletons.has(v.section)) continue;
        singletons.add(v.section);
      }
      used.add(t);
      const isPreview = !!v.preview && !preview && !v.pinned;
      if (isPreview) preview = true;
      views[t] = {
        id: t,
        section: v.section,
        pinned: !!v.pinned,
        preview: isPreview,
        ...(typeof v.param === 'string' ? { param: v.param } : {}),
        ...(typeof v.label === 'string' && v.label ? { label: v.label.slice(0, MAX_LABEL) } : {}),
      };
      tabs.push(t);
    }
    // Pinned tabs first.
    tabs.sort((a, b) => Number(views[b].pinned) - Number(views[a].pinned));
    const mru = (Array.isArray(n.mru) ? n.mru : []).filter((m, i, a): m is string => typeof m === 'string' && tabs.includes(m) && a.indexOf(m) === i);
    const active = typeof n.active === 'string' && tabs.includes(n.active) ? n.active : (mru[0] ?? tabs[0] ?? null);
    return { kind: 'group', id, tabs, active, mru };
  };

  const fix = (n: unknown, depth: number): LayoutNode | null => {
    if (!n || typeof n !== 'object' || depth > 8) return null;
    const node = n as Partial<LayoutNode>;
    if (node.kind === 'group') return fixGroup(node as Partial<GroupNode>);
    if (node.kind !== 'split') return null;
    const s = node as Partial<SplitNode>;
    const kids = (Array.isArray(s.children) ? s.children : []).map((c) => fix(c, depth + 1));
    const sizes = Array.isArray(s.sizes) ? s.sizes : [];
    const children = kids.filter((c): c is LayoutNode => !!c);
    const keptSizes = sizes.filter((_, i) => kids[i]);
    if (!children.length) return null;
    if (children.length === 1) return children[0];
    return flatten({ kind: 'split', id: typeof s.id === 'string' ? s.id : gen(), dir: s.dir === 'col' ? 'col' : 'row', children, sizes: normalizeSizes(keptSizes as number[], children.length) });
  };

  let tree = fix(r.tree, 0);
  if (!tree) return null;
  // Drop empty groups (keep one) and anything beyond the group limit.
  let l: Layout = { tree, views, activeGroup: typeof r.activeGroup === 'string' ? r.activeGroup : '' };
  for (const g of allGroups(l.tree)) if (!g.tabs.length) l = removeGroup(l, g.id);
  const gs = allGroups(l.tree);
  for (const g of gs.slice(MAX_GROUPS)) l = removeGroup(l, g.id);
  tree = l.tree;
  if (!allGroups(tree).some((g) => g.id === l.activeGroup)) l = { ...l, activeGroup: allGroups(tree)[0].id };
  if (!Object.keys(l.views).length) return null;
  return l;
}

// ---------- drag and drop ----------

export type DropZone = 'center' | Side;

/** Where a tab dropped at (x, y) over a group's content lands: an edge (split) or the centre (move). */
export function dropZone(x: number, y: number, rect: { left: number; top: number; width: number; height: number }): DropZone {
  const fx = (x - rect.left) / Math.max(1, rect.width);
  const fy = (y - rect.top) / Math.max(1, rect.height);
  const edge = 0.25;
  // The nearest edge wins when the pointer is inside an edge band.
  const d = [
    { z: 'left' as const, v: fx },
    { z: 'right' as const, v: 1 - fx },
    { z: 'up' as const, v: fy },
    { z: 'down' as const, v: 1 - fy },
  ].sort((a, b) => a.v - b.v)[0];
  return d.v < edge ? d.z : 'center';
}

/** Opens a section in a new group on `side` of `targetGroupId` (a sidebar item dropped on a group edge). */
export function openInSplit(l: Layout, spec: ViewSpec, targetGroupId: string, side: Side, gen: IdGen): Layout {
  const existing = isSingleton(spec.section) ? findViews(l, (v) => v.section === spec.section)[0] : undefined;
  if (existing) return moveViewToSplit(l, existing.id, targetGroupId, side, gen);
  const s = splitGroup(l, targetGroupId, side, gen);
  if (!s.groupId) return openSection(l, spec.section, spec.param, { group: targetGroupId, preview: false }, gen);
  return insertView(s.layout, s.groupId, spec, { preview: false }, gen).layout;
}
