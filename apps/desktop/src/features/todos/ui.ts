import { create } from 'zustand';
import type { DropPos } from './lib';

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
