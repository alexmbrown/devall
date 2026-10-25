import { create } from 'zustand';
import { History, Redo2, Trash2, Undo2 } from 'lucide-react';
import { Menu, type MenuItemDef } from '@/components/ui/Menu';
import { MOD, isMac, relTime } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type UndoHistory, type UndoOutcome } from '@/lib/api/git';

/**
 * Multi-level undo/redo of Source control operations (`devall_core::repos::undo`): the journal of
 * the selected repository, Undo/Redo with the confirmation the backend asks for when the
 * repository moved on, and refusals explained (e.g. a commit that was pushed since).
 */

export type Direction = 'undo' | 'redo';

/** What to tell the user about an outcome. Pure. */
export function outcomeMessage(o: UndoOutcome, dir: Direction): { kind: 'success' | 'info' | 'error'; title: string; detail?: string } | null {
  const verb = dir === 'undo' ? 'Undo' : 'Redo';
  switch (o.type) {
    case 'done':
      return { kind: 'success', title: `${dir === 'undo' ? 'Undone' : 'Redone'}: ${o.label}` };
    case 'refused':
      return { kind: 'error', title: `Can't ${dir} “${o.label}”`, detail: o.reason };
    case 'nothing':
      return { kind: 'info', title: `Nothing to ${dir}` };
    case 'confirm':
      return { kind: 'info', title: `${verb} “${o.label}”?`, detail: o.reasons.join('; ') };
  }
}

const keyOf = (root: string, repo: string | null) => `${root}|${repo ?? ''}`;

interface UndoState {
  key: string | null;
  history: UndoHistory | null;
  busy: boolean;
  load(root: string, repo: string | null): Promise<void>;
  /** Undoes/redoes `times` entries (stops at the first that needs a decision or is refused). */
  step(root: string, repo: string | null, dir: Direction, times?: number, onChanged?: () => void): Promise<void>;
  clear(root: string, repo: string | null): Promise<void>;
}

export const useUndo = create<UndoState>((set, get) => ({
  key: null,
  history: null,
  busy: false,

  async load(root, repo) {
    const key = keyOf(root, repo);
    try {
      const history = await gitApi.undoHistory(root, repo);
      set({ key, history });
    } catch {
      if (get().key !== key) set({ key, history: null });
    }
  },

  async step(root, repo, dir, times = 1, onChanged) {
    if (get().busy) return;
    set({ busy: true });
    let changed = false;
    try {
      for (let i = 0; i < times; i++) {
        const call = (force: boolean) => (dir === 'undo' ? gitApi.undo(root, repo, force) : gitApi.redo(root, repo, force));
        let o = await call(false);
        if (o.type === 'confirm') {
          const ok = await confirmDialog({
            title: `${dir === 'undo' ? 'Undo' : 'Redo'} “${o.label}” anyway?`,
            message: (
              <span className="col-sm">
                <span className="small">The repository changed since:</span>
                <ul className="small">
                  {o.reasons.map((r) => (
                    <li key={r}>{r}</li>
                  ))}
                </ul>
                <span className="small subtle">Only what this operation changed is put back. What it replaces is kept: {dir === 'undo' ? 'Redo' : 'Undo'} brings it back.</span>
              </span>
            ),
            confirmText: dir === 'undo' ? 'Undo anyway' : 'Redo anyway',
          });
          if (!ok) break;
          o = await call(true);
        }
        const m = outcomeMessage(o, dir);
        if (o.type === 'done') changed = true;
        if (o.type !== 'done') {
          if (m && o.type !== 'confirm') (m.kind === 'error' ? toast.error(m.title, m.detail) : toast.info(m.title, m.detail));
          break;
        }
        if (i === times - 1 && m) {
          const other: Direction = dir === 'undo' ? 'redo' : 'undo';
          toast.success(times > 1 ? `${dir === 'undo' ? 'Undone' : 'Redone'} ${times} operations` : m.title, undefined, { label: other === 'redo' ? 'Redo' : 'Undo', run: () => void get().step(root, repo, other, times, onChanged) });
        }
      }
    } catch (e) {
      toast.error(`${dir === 'undo' ? 'Undo' : 'Redo'} failed`, errorMessage(e));
    } finally {
      set({ busy: false });
      await get().load(root, repo);
      if (changed) onChanged?.();
    }
  },

  async clear(root, repo) {
    const ok = await confirmDialog({ title: 'Clear the undo history?', message: 'Forgets every recorded operation of this repository and removes their snapshots (refs/devall/undo/…). Your branches and files stay as they are.', confirmText: 'Clear history', danger: true });
    if (!ok) return;
    try {
      await gitApi.undoClear(root, repo);
    } catch (e) {
      toast.error('Could not clear the history', errorMessage(e));
    }
    await get().load(root, repo);
  },
}));

const redoKey = isMac ? '⇧⌘Z' : 'Ctrl+Shift+Z';

/** Undo / Redo buttons with the history list (click an entry to undo back to it). */
export function UndoButtons({ root, repo, onChanged }: { root: string; repo: string | null; onChanged(): void }) {
  const history = useUndo((s) => (s.key === keyOf(root, repo) ? s.history : null));
  const busy = useUndo((s) => s.busy);
  const step = useUndo((s) => s.step);
  const next = history?.undo[0];
  const redo = history?.redo[0];
  const items = (): MenuItemDef[] => {
    const list: MenuItemDef[] = [{ heading: 'Undo history' }];
    if (!history || (!history.undo.length && !history.redo.length)) list.push({ label: <span className="subtle small">Nothing recorded yet</span>, disabled: true });
    history?.redo
      .slice()
      .reverse()
      .forEach((e, i, arr) =>
        list.push({ label: <span className="truncate subtle">{e.label}</span>, icon: <Redo2 />, hint: relTime(e.at * 1000), onSelect: () => void step(root, repo, 'redo', arr.length - i, onChanged) }),
      );
    history?.undo.forEach((e, i) =>
      list.push({
        label: (
          <span className="truncate">
            {e.label}
            {e.pending ? <span className="subtle"> (in progress)</span> : null}
          </span>
        ),
        icon: <Undo2 />,
        hint: relTime(e.at * 1000),
        onSelect: () => void step(root, repo, 'undo', i + 1, onChanged),
      }),
    );
    if (history && (history.undo.length || history.redo.length)) list.push({ separator: true }, { label: 'Clear history…', icon: <Trash2 />, danger: true, onSelect: () => void useUndo.getState().clear(root, repo) });
    return list;
  };
  return (
    <>
      <button className="icon-btn sm" disabled={busy || !next} title={next ? `Undo “${next.label}” (${MOD}+Z)` : 'Nothing to undo'} onClick={() => void step(root, repo, 'undo', 1, onChanged)}>
        <Undo2 />
      </button>
      <button className="icon-btn sm" disabled={busy || !redo} title={redo ? `Redo “${redo.label}” (${redoKey})` : 'Nothing to redo'} onClick={() => void step(root, repo, 'redo', 1, onChanged)}>
        <Redo2 />
      </button>
      <Menu
        align="right"
        items={items}
        trigger={(p) => (
          <button className="icon-btn sm" title="Undo history — click an entry to undo back to it" {...p}>
            <History />
          </button>
        )}
      />
    </>
  );
}
