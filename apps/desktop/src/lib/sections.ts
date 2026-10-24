/**
 * The app's modules ("sections"): ids, labels and how the workbench may host them.
 * Pure data (no React, no stores) so the layout model and its tests can import it.
 * Icons live in `components/layout/sectionIcons.tsx`, page components in `components/workbench/ViewHost.tsx`.
 */

export type Section =
  | 'http'
  | 'environments'
  | 'utilities'
  | 'notes'
  | 'todos'
  | 'fakedata'
  | 'git'
  | 'projects'
  | 'settings'
  | 'plugins'
  | 'mocks'
  | 'inspector'
  | 'webhooks'
  | 'tunnels'
  | 'load'
  | 'data'
  | 'infra'
  | 'ai'
  | 'logs'
  | 'history';

export interface SectionMeta {
  id: Section;
  /** Sidebar / tab label. */
  label: string;
  /** Longer name for the command palette (defaults to `label`). */
  title?: string;
  /** Shows the projects page while no project is open. */
  needsProject?: boolean;
  /**
   * At most one view of this section exists: opening it again focuses the existing tab. Used for
   * modules whose "current selection" lives in a global store (mock server sub-navigation, the
   * inspector's capture selection, the log viewer's active source, Docker/K8s UI state…), so two
   * views could not show different things anyway.
   */
  singleton?: boolean;
  /**
   * The view's `param` names a document (a request path, a note path): different params are
   * different tabs, and opening one uses preview-tab semantics like an editor.
   */
  doc?: boolean;
}

export const SECTIONS: SectionMeta[] = [
  { id: 'http', label: 'Requests', needsProject: true, doc: true },
  { id: 'environments', label: 'Environments', needsProject: true },
  { id: 'utilities', label: 'Utilities' },
  { id: 'fakedata', label: 'Fake data' },
  { id: 'load', label: 'Load testing', needsProject: true },
  { id: 'mocks', label: 'Mock server', needsProject: true, singleton: true },
  { id: 'inspector', label: 'Inspector', title: 'Traffic inspector', singleton: true },
  { id: 'webhooks', label: 'Webhooks', needsProject: true },
  { id: 'logs', label: 'Logs', singleton: true },
  { id: 'history', label: 'History', title: 'History (requests, files, utilities, activity)', singleton: true },
  { id: 'infra', label: 'Docker & K8s', title: 'Docker & Kubernetes', singleton: true },
  { id: 'data', label: 'Databases' },
  { id: 'tunnels', label: 'Tunnels', singleton: true },
  { id: 'notes', label: 'Notes', needsProject: true, doc: true },
  { id: 'todos', label: 'To-dos', singleton: true },
  { id: 'git', label: 'Source control', needsProject: true, singleton: true },
  { id: 'ai', label: 'Local AI', singleton: true },
  { id: 'projects', label: 'Projects', singleton: true },
  { id: 'settings', label: 'Settings', singleton: true },
  { id: 'plugins', label: 'Plugins', singleton: true },
];

const BY_ID = new Map(SECTIONS.map((s) => [s.id, s]));

export const SECTION_IDS: Section[] = SECTIONS.map((s) => s.id);

export function sectionMeta(id: Section): SectionMeta {
  return BY_ID.get(id) ?? { id, label: id };
}

export function isSection(v: unknown): v is Section {
  return typeof v === 'string' && BY_ID.has(v as Section);
}

export const isSingleton = (id: Section) => !!BY_ID.get(id)?.singleton;
export const isDocSection = (id: Section) => !!BY_ID.get(id)?.doc;

// ---------- sidebar layout (user-customizable, persisted in prefs) ----------

export interface SidebarGroup {
  id: string;
  /** Empty = no header (the default first group). */
  label: string;
  items: Section[];
}

export interface SidebarLayout {
  groups: SidebarGroup[];
  /** Sections not shown in the sidebar (still reachable from "More…", the palette and links). */
  hidden: Section[];
}

const APP_SECTIONS: Section[] = ['projects', 'settings', 'plugins'];

export function defaultSidebar(): SidebarLayout {
  return {
    groups: [
      { id: 'main', label: '', items: SECTION_IDS.filter((s) => !APP_SECTIONS.includes(s)) },
      { id: 'app', label: 'App', items: [...APP_SECTIONS] },
    ],
    hidden: [],
  };
}

/**
 * Repairs a persisted sidebar layout: every known section appears exactly once (sections added in
 * newer versions go to the end of the first group), unknown ids and empty duplicate groups are dropped.
 */
export function normalizeSidebar(input: unknown): SidebarLayout {
  const def = defaultSidebar();
  if (!input || typeof input !== 'object') return def;
  const raw = input as Partial<SidebarLayout>;
  const seen = new Set<Section>();
  const groups: SidebarGroup[] = [];
  const ids = new Set<string>();
  for (const g of Array.isArray(raw.groups) ? raw.groups : []) {
    if (!g || typeof g !== 'object' || typeof g.id !== 'string' || ids.has(g.id)) continue;
    ids.add(g.id);
    const items = (Array.isArray(g.items) ? g.items : []).filter((s): s is Section => isSection(s) && !seen.has(s) && (seen.add(s), true));
    groups.push({ id: g.id, label: typeof g.label === 'string' ? g.label : '', items });
  }
  if (!groups.length) return def;
  const missing = SECTION_IDS.filter((s) => !seen.has(s));
  if (missing.length) {
    // Put new sections where the default puts them, when that group still exists.
    for (const s of missing) {
      const home = def.groups.find((g) => g.items.includes(s))?.id;
      (groups.find((g) => g.id === home) ?? groups[0]).items.push(s);
    }
  }
  const hidden = (Array.isArray(raw.hidden) ? raw.hidden : []).filter((s, i, a): s is Section => isSection(s) && a.indexOf(s) === i);
  return { groups, hidden };
}

/** Moves `section` to `index` within group `groupId` (removing it from wherever it was). */
export function moveSidebarItem(layout: SidebarLayout, section: Section, groupId: string, index: number): SidebarLayout {
  const groups = layout.groups.map((g) => ({ ...g, items: g.items.filter((s) => s !== section) }));
  const target = groups.find((g) => g.id === groupId);
  if (!target) return layout;
  const i = Math.max(0, Math.min(index, target.items.length));
  target.items.splice(i, 0, section);
  return { ...layout, groups };
}

/** Moves group `groupId` to `index` among the groups. */
export function moveSidebarGroup(layout: SidebarLayout, groupId: string, index: number): SidebarLayout {
  const from = layout.groups.findIndex((g) => g.id === groupId);
  if (from < 0) return layout;
  const groups = [...layout.groups];
  const [g] = groups.splice(from, 1);
  groups.splice(Math.max(0, Math.min(index, groups.length)), 0, g);
  return { ...layout, groups };
}

export function setSectionHidden(layout: SidebarLayout, section: Section, hidden: boolean): SidebarLayout {
  const rest = layout.hidden.filter((s) => s !== section);
  return { ...layout, hidden: hidden ? [...rest, section] : rest };
}

/** Drops `section` next to `ref` (before or after it) in group `groupId`, or at the group's end. */
export function placeSidebarItem(layout: SidebarLayout, section: Section, groupId: string, ref: { before?: Section; after?: Section } = {}): SidebarLayout {
  const g = layout.groups.find((x) => x.id === groupId);
  if (!g || ref.before === section || ref.after === section) return layout;
  const items = g.items.filter((s) => s !== section);
  const at = ref.before && items.includes(ref.before) ? items.indexOf(ref.before) : ref.after && items.includes(ref.after) ? items.indexOf(ref.after) + 1 : items.length;
  return moveSidebarItem(layout, section, groupId, at);
}
