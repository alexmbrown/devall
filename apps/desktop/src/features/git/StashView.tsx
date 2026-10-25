import { useEffect, useState } from 'react';
import { Archive, ArrowDownToLine, FileWarning, Trash2, X } from 'lucide-react';
import { Empty, RelTime, Spinner } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type ChangedFile, type DiffSide, type StashEntry, type StashFiles } from '@/lib/api/git';
import * as op from './branchOps';
import { ChangedFileList } from './ChangedFileList';
import { useCompare } from './compareStore';
import { DiffPane } from './DiffPane';
import { displayOrder } from './review';

/** A stash entry's changes (tracked files against where it was made, untracked files as added), with Apply / Pop / Drop. */
export function StashView({ ctx, entry, onClose }: { ctx: op.GitCtx; entry: StashEntry; onClose(): void }) {
  const [data, setData] = useState<StashFiles | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const opts = useCompare((s) => s.opts);

  useEffect(() => {
    let alive = true;
    setData(null);
    setError(null);
    gitApi
      .stashFiles(ctx.root, ctx.repo, entry.index, entry.oid)
      .then((d) => {
        if (!alive) return;
        setData(d);
        setSel(d.files[0]?.path ?? null);
      })
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [ctx.root, ctx.repo, entry.index, entry.oid]);

  const file = data?.files.find((f) => f.path === sel) ?? null;
  const sides = (f: ChangedFile): { old: DiffSide; next: DiffSide } => {
    if (f.untracked && data?.untrackedCommit) return { old: { type: 'empty' }, next: { type: 'rev', rev: data.untrackedCommit, path: f.path } };
    return { old: f.status === 'A' ? { type: 'empty' } : { type: 'rev', rev: data!.base, path: f.oldPath ?? f.path }, next: f.status === 'D' ? { type: 'empty' } : { type: 'rev', rev: data!.commit, path: f.path } };
  };
  const order = data ? displayOrder(data.files, false) : { main: [], generated: [] };
  return (
    <div className="col h-full min-h-0" style={{ gap: 0 }}>
      <div className="toolbar">
        <Archive size={14} className="subtle" />
        <span className="small strong truncate">{entry.title}</span>
        <span className="xs subtle shrink-0">
          {entry.name}
          {entry.branch ? ` · on ${entry.branch}` : ''} · <RelTime ts={entry.time * 1000} />
        </span>
        <span className="spacer" />
        <button className="btn btn-sm" onClick={() => void op.applyStash(ctx, entry, false)}>
          <ArrowDownToLine /> Apply
        </button>
        <button className="btn btn-sm" onClick={() => void op.applyStash(ctx, entry, true).then(onClose)}>
          Pop
        </button>
        <button className="btn btn-sm btn-danger-ghost" onClick={() => void op.dropStash(ctx, entry).then(onClose)}>
          <Trash2 /> Drop…
        </button>
        <button className="icon-btn sm" title="Close" onClick={onClose}>
          <X />
        </button>
      </div>
      {error ? (
        <Empty icon={<FileWarning />} title="Couldn't read the stash">{error}</Empty>
      ) : !data ? (
        <Empty icon={<Spinner />} title="Reading the stash…" />
      ) : (
        <div className="git-history">
          <div className="git-history-list" style={{ width: 300 }}>
            <ChangedFileList main={order.main} generated={order.generated} tree={false} selected={sel} onSelect={setSel} showGenerated onToggleGenerated={() => {}} />
          </div>
          <div className="git-history-diff">
            {file ? (
              <div className="grow min-h-0 git-review-diff">
                <DiffPane root={ctx.root} repo={ctx.repo} old={sides(file).old} next={sides(file).next} path={file.path} lines={(file.added ?? 0) + (file.deleted ?? 0)} opts={opts} />
              </div>
            ) : (
              <Empty title="Empty stash" />
            )}
          </div>
        </div>
      )}
    </div>
  );
}
