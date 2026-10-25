import { useEffect, useMemo, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { GitBranch, GripVertical, ListOrdered, RotateCcw, Wand2 } from 'lucide-react';
import { Modal } from '@/components/ui/Modal';
import { Callout, Checkbox, Spinner } from '@/components/ui/misc';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type RebaseAction, type RebaseTodo } from '@/lib/api/git';
import { reportOp } from './branchOps';
import { ACTIONS, actionForKey, defaultMessage, fromSteps, headOf, initialPlan, isNoop, messageOf, moveRow, needsMessage, planSummary, toSteps, validatePlan, type PlanRow } from './rebasePlan';

type Open = { root: string; repo: string | null; base: string; label: string; onDone(): void } | null;

export const useRebaseDialog = create<{ open: Open; show(o: Open): void }>((set) => ({ open: null, show: (open) => set({ open }) }));

/** Opens the interactive rebase of the checked-out branch: the commits after `base`. */
export const openInteractiveRebase = (o: NonNullable<Open>) => useRebaseDialog.getState().show(o);

export function RebaseDialogHost() {
  const open = useRebaseDialog((s) => s.open);
  if (!open) return null;
  return <RebaseDialog key={`${open.repo}|${open.base}`} {...open} onClose={() => useRebaseDialog.getState().show(null)} />;
}

/**
 * Interactive rebase, visual: the commits oldest first (git's replay order), drag to reorder (or
 * Alt+↑/↓), an action per commit (keys p r e s f d on the focused row), messages for rewords and
 * squash groups, autosquash (`fixup!`/`squash!` commits move under their target) and
 * `--update-refs` for stacked branches. One undo entry; conflicts and edit stops pause it.
 */
function RebaseDialog({ root, repo, base, label, onDone, onClose }: NonNullable<Open> & { onClose(): void }) {
  const [todo, setTodo] = useState<RebaseTodo | null>(null);
  const [rows, setRows] = useState<PlanRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [updateRefs, setUpdateRefs] = useState(true);
  const [focus, setFocus] = useState(0);
  const [drag, setDrag] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<number | null>(null);

  useEffect(() => {
    gitApi
      .rebaseTodo(root, repo, base)
      .then((t) => {
        setTodo(t);
        setRows(initialPlan(t));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [root, repo, base]);

  const original = useMemo(() => todo?.commits.map((c) => c.hash) ?? [], [todo]);
  const problem = rows.length ? validatePlan(rows) : null;
  const noop = rows.length ? isNoop(rows, original) : true;
  const stacked = rows.some((r) => r.branches.length);

  const setAction = (i: number, action: RebaseAction) => setRows((rs) => rs.map((r, k) => (k === i ? { ...r, action } : r)));
  const setMessage = (i: number, message: string) => setRows((rs) => rs.map((r, k) => (k === i ? { ...r, message } : r)));
  const move = (from: number, to: number) => {
    setRows((rs) => moveRow(rs, from, to));
    setFocus(to);
  };

  const autosquash = async () => {
    if (!todo) return;
    try {
      const steps = await gitApi.rebaseAutosquash(root, repo, base, toSteps(rows));
      const next = fromSteps(rows, steps);
      if (next.every((r, i) => r.hash === rows[i].hash && r.action === rows[i].action)) toast.info('Nothing to autosquash', 'No fixup!, squash! or amend! commits name an earlier commit.');
      setRows(next);
    } catch (e) {
      toast.error('Autosquash failed', errorMessage(e));
    }
  };

  const start = async () => {
    if (!todo || problem) return;
    setBusy(true);
    try {
      const res = await gitApi.rebaseInteractive(root, repo, base, toSteps(rows), updateRefs && todo.canUpdateRefs);
      reportOp('Rebase', res, `Rebased ${todo.branch ?? 'HEAD'} (${planSummary(rows)})`);
      onClose();
    } catch (e) {
      toast.error('Rebase failed', errorMessage(e));
    } finally {
      setBusy(false);
      onDone();
    }
  };

  const onRowKey = (e: React.KeyboardEvent, i: number) => {
    if ((e.target as HTMLElement).tagName === 'TEXTAREA' || (e.target as HTMLElement).tagName === 'SELECT') return;
    if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      e.preventDefault();
      const to = i + (e.key === 'ArrowUp' ? -1 : 1);
      if (to >= 0 && to < rows.length) move(i, to);
      return;
    }
    if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      setFocus(Math.max(0, Math.min(rows.length - 1, i + (e.key === 'ArrowUp' ? -1 : 1))));
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const a = actionForKey(e.key);
    if (a) {
      e.preventDefault();
      setAction(i, a);
    }
  };

  // Keyboard focus follows the focused row — unless something inside it (its select, its
  // message box) already has it.
  useEffect(() => {
    const el = document.querySelector<HTMLElement>(`.git-rebase-row[data-i="${focus}"]`);
    if (el && !el.contains(document.activeElement)) el.focus();
  }, [focus, rows.length]);

  return (
    <Modal
      open
      onClose={busy ? () => {} : onClose}
      size="xl"
      title={
        <span className="row-sm">
          <ListOrdered size={16} /> Interactive rebase{todo ? ` — ${todo.branch ?? 'HEAD'} onto ${label}` : ''}
        </span>
      }
      footer={
        <>
          <span className="xs subtle grow">{rows.length ? planSummary(rows) : ''}</span>
          <button className="btn" disabled={busy} onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={busy || !!problem || !todo || (noop && base === todo.base && !rows.length)} onClick={() => void start()} title={problem ?? undefined}>
            {busy ? <Spinner /> : <ListOrdered />} Start rebase
          </button>
        </>
      }
    >
      {error ? (
        <Callout kind="danger">{error}</Callout>
      ) : !todo ? (
        <div className="row-sm subtle small">
          <Spinner /> Reading the commits…
        </div>
      ) : todo.hasMerges ? (
        <Callout kind="warning">The commits after {label} include merge commits — this editor handles straight histories. Rebase onto a later commit, or use a terminal for --rebase-merges.</Callout>
      ) : (
        <div className="col">
          <div className="row-sm wrap">
            <span className="small grow">
              {rows.length} commit{rows.length === 1 ? '' : 's'} replayed onto <span className="mono">{todo.baseShort}</span> ({label}), top to bottom. Drag to reorder (Alt+↑/↓); keys <span className="mono">p r e s f d</span> set the action.
            </span>
            <button className="btn btn-sm" onClick={() => void autosquash()} title="Move fixup!/squash!/amend! commits under the commit they name">
              <Wand2 /> Autosquash
            </button>
            <button className="btn btn-sm btn-ghost" onClick={() => setRows(initialPlan(todo))} title="Start over">
              <RotateCcw /> Reset
            </button>
          </div>
          {todo.canUpdateRefs && stacked && (
            <Checkbox checked={updateRefs} onChange={setUpdateRefs} label={<span className="small">Update stacked branches (--update-refs): branches pointing at these commits move to the rewritten ones</span>} />
          )}
          {problem && <Callout kind="warning">{problem}</Callout>}
          <div className="git-rebase-list" role="listbox" aria-label="Commits to replay">
            {rows.map((r, i) => {
              const head = headOf(rows, i);
              const msg = needsMessage(rows, i);
              return (
                <div
                  key={r.hash}
                  data-i={i}
                  tabIndex={0}
                  role="option"
                  aria-selected={focus === i}
                  className={clsx('git-rebase-row', `act-${r.action}`, focus === i && 'focus', drag === i && 'dragging', head >= 0 && 'melded')}
                  draggable
                  onDragStart={(e) => {
                    setDrag(i);
                    e.dataTransfer.effectAllowed = 'move';
                    e.dataTransfer.setData('text/plain', String(i));
                  }}
                  onDragOver={(e) => {
                    e.preventDefault();
                    if (drag !== null && drag !== i) {
                      move(drag, i);
                      setDrag(i);
                    }
                  }}
                  onDragEnd={() => setDrag(null)}
                  onFocus={() => setFocus(i)}
                  onKeyDown={(e) => onRowKey(e, i)}
                >
                  <div className="git-rebase-line">
                    <GripVertical size={14} className="subtle git-rebase-grip" />
                    <select className="select git-rebase-action" value={r.action} onChange={(e) => setAction(i, e.target.value as RebaseAction)} title={ACTIONS.find((a) => a.value === r.action)?.title}>
                      {ACTIONS.map((a) => (
                        <option key={a.value} value={a.value} title={a.title}>
                          {a.label} ({a.key})
                        </option>
                      ))}
                    </select>
                    <span className="mono xs subtle shrink-0">{r.short}</span>
                    <span className={clsx('small truncate grow', r.action === 'drop' && 'git-struck')}>{r.subject}</span>
                    {r.branches.map((b) => (
                      <span key={b} className="git-pill git-pill-branch" title="Another branch points here">
                        <GitBranch size={10} />
                        {b}
                      </span>
                    ))}
                    {head >= 0 && <span className="xs subtle shrink-0">into {rows[head].short}</span>}
                    {msg && (
                      <button className="btn btn-sm btn-ghost" onClick={() => setEditing(editing === i ? null : i)}>
                        {editing === i ? 'Done' : 'Message…'}
                      </button>
                    )}
                  </div>
                  {msg && editing === i && (
                    <div className="git-rebase-msg">
                      <textarea className="textarea mono" rows={Math.min(10, Math.max(3, messageOf(rows, i).split('\n').length + 1))} value={messageOf(rows, i)} onChange={(e) => setMessage(i, e.target.value)} />
                      {r.message !== null && r.message !== defaultMessage(rows, i) && (
                        <button className="btn btn-sm btn-ghost" onClick={() => setRows((rs) => rs.map((x, k) => (k === i ? { ...x, message: null } : x)))}>
                          Use the default message
                        </button>
                      )}
                    </div>
                  )}
                  {msg && editing !== i && <div className="xs subtle git-rebase-msg-preview truncate">{messageOf(rows, i).split('\n')[0]}</div>}
                </div>
              );
            })}
          </div>
          <span className="xs subtle">Edit stops after its commit so you can amend it; then Continue in the banner. Undo (Ctrl/Cmd+Z) takes the whole rebase back.</span>
        </div>
      )}
    </Modal>
  );
}
