import { create } from 'zustand';
import { groupChoices, type GroupBy, type Layout, type SubtaskMode } from './boardModel';
import { EMPTY_FILTER, normalizeFilter, type TaskFilter, type ViewStats } from './filterModel';
import { viewKey, type DropPos, type SortKey, type ViewRef } from './lib';

/** Something other than tasks being dragged: a section (header/column), a list (left pane, board column), a status column or a saved filter. */
export type DragItem = { kind: 'section' | 'list' | 'column' | 'filter'; id: string };

export type { ViewStats };

/** Page-level UI state of the to-do page: selection, keyboard cursor, inline edit, drag & drop. */
interface UiState {
  /** Selected task ids (multi-select with Ctrl/Cmd/Shift). */
  sel: string[];
  /** Shift-click anchor. */
  anchor: string | null;
  /** Keyboard cursor (j/k). */
  cursor: string | null;
  /** Task being edited inline. */
  editing: string | null;
  /** Tasks being dragged. */
  drag: string[] | null;
  /** Where they'd land. */
  drop: { id: string; pos: DropPos } | null;
  /** A section or list being dragged (reordering). */
  dragItem: DragItem | null;
  /** Row ids of the current view in display order (for j/k and Shift-click). */
  order: string[];
  /** A board's card ids per visible column (arrow keys), with the columns' keys. */
  grid: { keys: string[]; ids: string[][] } | null;
  /** Card whose quick editor is open (board: hover pencil, `e`). */
  quickEdit: string | null;
  /** A picker opened from the keyboard on a card (board: `l`, `d`). */
  cardPicker: { id: string; kind: 'labels' | 'due' } | null;
  /** The current view's counts and hidden things. */
  stats: ViewStats | null;
  /** Search text (left pane). */
  search: string;
  /** Show completed tasks in list views. */
  showDone: boolean;
  /** Which inline "Add task" composer is open (`slot` key). */
  composer: string | null;
  quickAdd: boolean;
  set(p: Partial<UiState>): void;
}

export const useTodoUi = create<UiState>((set) => ({
  sel: [],
  anchor: null,
  cursor: null,
  editing: null,
  drag: null,
  drop: null,
  dragItem: null,
  order: [],
  grid: null,
  quickEdit: null,
  cardPicker: null,
  stats: null,
  search: '',
  showDone: false,
  composer: null,
  quickAdd: false,
  set: (p) => set(p),
}));

/** The ids an action applies to: the selection, else the cursor row. */
export function targets(): string[] {
  const { sel, cursor } = useTodoUi.getState();
  return sel.length ? sel : cursor ? [cursor] : [];
}

/**
 * Starts dragging tasks or an item. The store is updated after `dragstart` returns: re-rendering
 * the dragged element inside `dragstart` cancels the drag in Chromium/WebView2 (see the workbench).
 */
export function beginDrag(e: React.DragEvent, p: { tasks: string[] } | { item: DragItem }) {
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', 'tasks' in p ? p.tasks.join(',') : `${p.item.kind}:${p.item.id}`);
  // Several tasks: a "3 tasks" chip follows the pointer instead of one card.
  if ('tasks' in p && p.tasks.length > 1 && typeof document !== 'undefined') {
    const ghost = document.createElement('div');
    ghost.className = 'td-drag-ghost';
    ghost.textContent = `${p.tasks.length} tasks`;
    document.body.appendChild(ghost);
    e.dataTransfer.setDragImage(ghost, 12, 12);
    setTimeout(() => ghost.remove(), 0);
  }
  setTimeout(() => useTodoUi.getState().set('tasks' in p ? { drag: p.tasks, dragItem: null } : { dragItem: p.item, drag: null }), 0);
}

/** Ends any drag (also on `dragend`, so nothing is left highlighted when a drop misses). */
export function endDrag() {
  const s = useTodoUi.getState();
  if (s.drag || s.drop || s.dragItem) s.set({ drag: null, drop: null, dragItem: null });
}

// ── per-view options (layout, board grouping, sort), remembered on this machine ────────────────

export interface ViewOptions {
  layout: Layout;
  groupBy: GroupBy;
  sort: SortKey;
  /** Board: sub-tasks as their own cards, or inside their parent's card. */
  subtasks: SubtaskMode;
  /** Board: background tint (palette name or `#hex`), none by default. */
  accent: string | null;
}

const OPTIONS_KEY = 'devall.todos.viewOptions.v2';
/** Before status columns (boards grouped by section/list by default). */
const OLD_OPTIONS_KEY = 'devall.todos.viewOptions';

function storedOptions(): Record<string, Partial<ViewOptions>> {
  try {
    const raw = localStorage.getItem(OPTIONS_KEY);
    if (raw) {
      const v = JSON.parse(raw);
      return v && typeof v === 'object' ? v : {};
    }
    // Keep layouts and sorting from before; every board starts over on the new default grouping (Status).
    const old = JSON.parse(localStorage.getItem(OLD_OPTIONS_KEY) ?? '{}');
    const out: Record<string, Partial<ViewOptions>> = {};
    if (old && typeof old === 'object')
      for (const [k, o] of Object.entries(old as Record<string, Partial<ViewOptions>>)) {
        if (!o || typeof o !== 'object') continue;
        const { groupBy: _drop, ...rest } = o;
        out[k] = rest;
      }
    return out;
  } catch {
    return {};
  }
}

export const useViewOptions = create<{ byView: Record<string, Partial<ViewOptions>>; set(view: ViewRef, p: Partial<ViewOptions>): void }>((set, get) => ({
  byView: storedOptions(),
  set(view, p) {
    const key = viewKey(view);
    const byView = { ...get().byView, [key]: { ...get().byView[key], ...p } };
    try {
      localStorage.setItem(OPTIONS_KEY, JSON.stringify(byView));
    } catch {
      /* private mode */
    }
    set({ byView });
  },
}));

/**
 * A view's options with defaults (list layout, the view's first grouping, manual order; sub-tasks
 * as cards in All tasks — every task gets a card there — and inside their parent's card elsewhere).
 */
export function resolveOptions(view: ViewRef, stored?: Partial<ViewOptions>): ViewOptions {
  const groups = groupChoices(view);
  return {
    layout: stored?.layout === 'board' && view.kind !== 'completed' ? 'board' : 'list',
    groupBy: stored?.groupBy && groups.includes(stored.groupBy) ? stored.groupBy : groups[0],
    sort: stored?.sort ?? 'manual',
    subtasks: stored?.subtasks === 'cards' || stored?.subtasks === 'nested' ? stored.subtasks : view.kind === 'all' ? 'cards' : 'nested',
    accent: typeof stored?.accent === 'string' ? stored.accent : null,
  };
}

export function useOptions(view: ViewRef): ViewOptions {
  const stored = useViewOptions((s) => s.byView[viewKey(view)]);
  return resolveOptions(view, stored);
}

// ── status boards: the done column (all views), remembered on this machine ──────────────────────

/**
 * How many days of completions the done column shows; 0 = every completed task (the snapshot has
 * the last 7 days — "all" loads the older ones too).
 */
export const DONE_DAYS = [1, 3, 7, 0] as const;

export const doneDaysLabel = (d: number) => (d === 0 ? 'All completed' : d === 1 ? 'Completed today' : `Completed in the last ${d} days`);

export interface BoardPrefs {
  doneDays: number;
  /** The done column folded to a narrow strip. */
  doneCollapsed: boolean;
  /** Other columns folded to strips (column keys, e.g. `status:review`, `p4`). */
  collapsed: string[];
  /** Card label strips show the label names (Trello: click a strip). */
  labelsExpanded: boolean;
}

const BOARD_KEY = 'devall.todos.board';

/** Board preferences from their stored JSON (anything missing or invalid: the defaults). */
export function storedBoard(raw: string | null): BoardPrefs {
  const d: BoardPrefs = { doneDays: 7, doneCollapsed: false, collapsed: [], labelsExpanded: false };
  try {
    const v = JSON.parse(raw ?? '{}') as Partial<BoardPrefs>;
    return {
      doneDays: (DONE_DAYS as readonly number[]).includes(v.doneDays as number) ? v.doneDays! : d.doneDays,
      doneCollapsed: v.doneCollapsed === true,
      collapsed: Array.isArray(v.collapsed) ? v.collapsed.filter((k): k is string => typeof k === 'string') : [],
      labelsExpanded: v.labelsExpanded === true,
    };
  } catch {
    return d;
  }
}

function readBoard(): string | null {
  try {
    return localStorage.getItem(BOARD_KEY);
  } catch {
    return null;
  }
}

export const useBoardPrefs = create<BoardPrefs & { set(p: Partial<BoardPrefs>): void }>((set, get) => ({
  ...storedBoard(readBoard()),
  set(p) {
    const { doneDays, doneCollapsed, collapsed, labelsExpanded } = get();
    const next = { doneDays, doneCollapsed, collapsed, labelsExpanded, ...p };
    try {
      localStorage.setItem(BOARD_KEY, JSON.stringify(next));
    } catch {
      /* private mode */
    }
    set(next);
  },
}));

// ── the filter bar, per view, for this session ──────────────────────────────────────────────────

const FILTER_KEY = 'devall.todos.filter.v1';

/**
 * Filters are remembered per view for the session (sessionStorage: they survive switching views
 * and tabs, not a restart — a forgotten filter must never make tasks look lost later).
 */
function storedFilters(): Record<string, TaskFilter> {
  try {
    const v = JSON.parse(sessionStorage.getItem(FILTER_KEY) ?? '{}');
    if (!v || typeof v !== 'object') return {};
    return Object.fromEntries(Object.entries(v as Record<string, Partial<TaskFilter>>).map(([k, f]) => [k, normalizeFilter(f)]));
  } catch {
    return {};
  }
}

export const useTaskFilter = create<{ byView: Record<string, TaskFilter>; set(view: ViewRef, p: Partial<TaskFilter>): void; clear(view: ViewRef): void }>((set, get) => {
  const save = (byView: Record<string, TaskFilter>) => {
    try {
      sessionStorage.setItem(FILTER_KEY, JSON.stringify(byView));
    } catch {
      /* private mode */
    }
    set({ byView });
  };
  return {
    byView: storedFilters(),
    set(view, p) {
      const key = viewKey(view);
      save({ ...get().byView, [key]: { ...(get().byView[key] ?? EMPTY_FILTER), ...p } });
    },
    clear(view) {
      const { [viewKey(view)]: _drop, ...rest } = get().byView;
      save(rest);
    },
  };
});

/** The view's filter (empty when none). */
export function useViewFilter(view: ViewRef): TaskFilter {
  return useTaskFilter((s) => s.byView[viewKey(view)]) ?? EMPTY_FILTER;
}
