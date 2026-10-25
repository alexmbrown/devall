import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { Columns2, FileWarning, Minus, Plus, Space, Undo2 } from 'lucide-react';
import { Empty, Spinner } from '@/components/ui/misc';
import { confirmDialog } from '@/components/ui/dialogs';
import { toast } from '@/components/ui/toast';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type LineAction, type LineSelection, type PatchView } from '@/lib/api/git';
import { pickLine, selectionCounts, toSelection, type Picked } from './staging';

/**
 * A changed file's diff as hunks with stage / unstage / discard per hunk or per selected lines
 * (click a changed line, Shift+click for a run). The patch is rebuilt and applied by git
 * (`git apply --cached --recount`, reversed for unstage/discard) — only if the file is still
 * exactly as shown. Discards ask first, naming the lines they remove.
 */
export function HunkView({
  root,
  repo,
  path,
  staged,
  version,
  ignoreWs,
  onIgnoreWs,
  onSideBySide,
  onChanged,
}: {
  root: string;
  repo: string | null;
  path: string;
  staged: boolean;
  /** Reload when this changes (status refreshes). */
  version: unknown;
  ignoreWs: boolean;
  onIgnoreWs(v: boolean): void;
  onSideBySide(): void;
  onChanged(): void;
}) {
  const [view, setView] = useState<PatchView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [force, setForce] = useState(false);
  const [picked, setPicked] = useState<Picked>({});
  const [busy, setBusy] = useState(false);
  const anchor = useRef<{ hunk: number; line: number } | null>(null);
  const token = useRef(0);

  useEffect(() => {
    setForce(false);
    setPicked({});
  }, [path, staged]);
  useEffect(() => {
    const t = ++token.current;
    gitApi
      .patch(root, repo, path, staged, ignoreWs, force)
      .then((v) => {
        if (t !== token.current) return;
        setView(v);
        setError(null);
        // Keep picked lines only while the diff is the same one.
        setPicked((p) => (view && v.fingerprint === view.fingerprint ? p : {}));
      })
      .catch((e) => t === token.current && setError(errorMessage(e)));
    // `view` is read to compare fingerprints, not watched.
  }, [root, repo, path, staged, ignoreWs, force, version]); // eslint-disable-line react-hooks/exhaustive-deps

  const apply = async (action: LineAction, sel: LineSelection[]) => {
    if (!view || !sel.length) return;
    if (action === 'discard') {
      const c = selectionCounts(view, sel);
      const what = [c.added && `${c.added} added line${c.added === 1 ? '' : 's'}`, c.deleted && `${c.deleted} deleted line${c.deleted === 1 ? '' : 's'}`].filter(Boolean).join(' and ');
      const ok = await confirmDialog({
        title: `Discard ${what} in ${path.split('/').pop()}?`,
        message: `The working-tree file goes back to the staged version for ${sel.length === 1 && sel[0].lines === null ? 'this hunk' : 'the selected lines'} (added lines are removed, deleted ones restored). This can't be undone.`,
        confirmText: 'Discard',
        danger: true,
      });
      if (!ok) return;
    }
    setBusy(true);
    try {
      await gitApi.applyLines(root, repo, path, action, view.fingerprint, sel, ignoreWs);
      setPicked({});
    } catch (e) {
      toast.error(`${action[0].toUpperCase()}${action.slice(1)} failed`, errorMessage(e));
    } finally {
      setBusy(false);
      onChanged();
    }
  };

  const toggle = (e: React.MouseEvent, hunk: number, line: number) => {
    if (!view) return;
    setPicked((p) => pickLine(view, p, hunk, line, e.shiftKey ? anchor.current : null));
    if (!e.shiftKey) anchor.current = { hunk, line };
  };

  const sel = view ? toSelection(view, picked) : [];
  const counts = view ? selectionCounts(view, sel) : { added: 0, deleted: 0 };
  const n = counts.added + counts.deleted;

  const toolbar = (
    <div className="toolbar git-diff-toolbar">
      <span className="mono xs truncate grow">{path}</span>
      <span className="badge outline shrink-0">{staged ? 'HEAD ↔ staged' : 'staged ↔ working tree'}</span>
      {view?.crlf && <span className="badge outline shrink-0" title="This file's lines end in CRLF; git handles the line endings when staging">CRLF</span>}
      {n > 0 && (
        <>
          <span className="xs subtle shrink-0">{n} selected</span>
          {staged ? (
            <button className="btn btn-sm" disabled={busy} onClick={() => void apply('unstage', sel)}>
              <Minus /> Unstage lines
            </button>
          ) : (
            <>
              <button className="btn btn-sm" disabled={busy} onClick={() => void apply('stage', sel)}>
                <Plus /> Stage lines
              </button>
              <button className="btn btn-sm btn-danger-ghost" disabled={busy} onClick={() => void apply('discard', sel)}>
                <Undo2 /> Discard lines
              </button>
            </>
          )}
          <button className="btn btn-sm btn-ghost" onClick={() => setPicked({})}>
            Clear
          </button>
        </>
      )}
      {busy && <Spinner />}
      <button className={clsx('icon-btn sm', ignoreWs && 'on')} title="Ignore whitespace (staging then ignores whitespace-only changes too)" onClick={() => onIgnoreWs(!ignoreWs)}>
        <Space />
      </button>
      <button className="icon-btn sm" title="Side by side" onClick={onSideBySide}>
        <Columns2 />
      </button>
    </div>
  );

  let content: React.ReactNode;
  if (error) content = <Empty icon={<FileWarning />} title="Couldn't read the diff">{error}</Empty>;
  else if (!view) content = <Empty icon={<Spinner />} title="Reading the diff…" />;
  else if (view.binary) content = <Empty icon={<FileWarning />} title="Binary file">Stage, unstage or discard it as a whole from the list.</Empty>;
  else if (view.tooLarge)
    content = (
      <Empty icon={<FileWarning />} title="Large diff" action={<button className="btn btn-sm" onClick={() => setForce(true)}>Load anyway</button>}>
        {view.lineCount.toLocaleString()} diff lines — not loaded automatically.
      </Empty>
    );
  else if (!view.hunks.length) content = <Empty title="No line changes">{ignoreWs ? 'Only whitespace changed.' : 'Mode or line-ending changes only.'}</Empty>;
  else
    content = (
      <div className="git-hunks">
        {view.hunks.map((h, hi) => (
          <div key={hi} className="git-hunk">
            <div className="git-hunk-head">
              <span className="mono xs truncate grow">{h.header}</span>
              {staged ? (
                <button className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void apply('unstage', [{ hunk: hi, lines: null }])}>
                  <Minus /> Unstage hunk
                </button>
              ) : (
                <>
                  <button className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void apply('stage', [{ hunk: hi, lines: null }])}>
                    <Plus /> Stage hunk
                  </button>
                  <button className="btn btn-sm btn-danger-ghost" disabled={busy} onClick={() => void apply('discard', [{ hunk: hi, lines: null }])}>
                    <Undo2 /> Discard hunk
                  </button>
                </>
              )}
            </div>
            <div className="git-hunk-lines">
              {h.lines.map((l, li) => (
                <div
                  key={li}
                  className={clsx('git-line', l.kind === '+' && 'add', l.kind === '-' && 'del', picked[hi]?.includes(li) && 'picked')}
                  onClick={(e) => toggle(e, hi, li)}
                  title={l.kind === ' ' ? undefined : 'Click to select, Shift+click for a run'}
                >
                  <span className="git-ln">{l.old ?? ''}</span>
                  <span className="git-ln">{l.new ?? ''}</span>
                  <span className="git-sign">{l.kind === ' ' ? '' : l.kind}</span>
                  <span className="git-code">
                    {l.text || ' '}
                    {l.noEol && <span className="git-noeol" title="No newline at end of file">⏎̸</span>}
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    );

  return (
    <div className="col h-full min-h-0" style={{ gap: 0 }}>
      {toolbar}
      <div className="grow min-h-0 git-hunk-scroll">{content}</div>
    </div>
  );
}
