import { create } from 'zustand';
import { groupChoices, type GroupBy, type Layout } from './boardModel';
import { viewKey, type DropPos, type SortKey, type ViewRef } from './lib';

/** Something other than tasks being dragged: a section (header/column), a list (left pane, board column), a status column or a saved filter. */
export type DragItem = { kind: 'section' | 'list' | 'column' | 'filter'; id: string };

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

/** A view's options with defaults (list layout, the view's first grouping, manual order). */
export function resolveOptions(view: ViewRef, stored?: Partial<ViewOptions>): ViewOptions {
  const groups = groupChoices(view);
  return {
    layout: stored?.layout === 'board' && view.kind !== 'completed' ? 'board' : 'list',
    groupBy: stored?.groupBy && groups.includes(stored.groupBy) ? stored.groupBy : groups[0],
    sort: stored?.sort ?? 'manual',
  };
}

export function useOptions(view: ViewRef): ViewOptions {
  const stored = useViewOptions((s) => s.byView[viewKey(view)]);
  return resolveOptions(view, stored);
}

// ── status boards: the done column (all views), remembered on this machine ──────────────────────

/** How many days of completions the done column shows (the snapshot has the last 7). */
export const DONE_DAYS = [1, 3, 7] as const;

export interface BoardPrefs {
  doneDays: number;
  /** The done column folded to a narrow strip. */
  doneCollapsed: boolean;
}

const BOARD_KEY = 'devall.todos.board';

function storedBoard(): BoardPrefs {
  const d: BoardPrefs = { doneDays: 7, doneCollapsed: false };
  try {
    const v = JSON.parse(localStorage.getItem(BOARD_KEY) ?? '{}') as Partial<BoardPrefs>;
    return { doneDays: DONE_DAYS.includes(v.doneDays as 1 | 3 | 7) ? v.doneDays! : d.doneDays, doneCollapsed: v.doneCollapsed === true };
  } catch {
    return d;
  }
}

export const useBoardPrefs = create<BoardPrefs & { set(p: Partial<BoardPrefs>): void }>((set, get) => ({
  ...storedBoard(),
  set(p) {
    const next = { doneDays: get().doneDays, doneCollapsed: get().doneCollapsed, ...p };
    try {
      localStorage.setItem(BOARD_KEY, JSON.stringify(next));
    } catch {
      /* private mode */
    }
    set(next);
  },
}));
