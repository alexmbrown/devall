import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { create } from 'zustand';
import clsx from 'clsx';
import { FileClock, FileWarning, History, ScanText, X } from 'lucide-react';
import { Empty, RelTime, Resizer, Segmented, Spinner, relTime } from '@/components/ui/misc';
import { errorMessage } from '@/lib/ipc';
import { gitApi, type Blame, type BlameCommit, type DiffSide, type FileVersion } from '@/lib/api/git';
import { useViewState } from '@/store/nav';
import { StatusLetter } from './ChangedFileList';
import { useCompare } from './compareStore';
import { DiffPane } from './DiffPane';

export type FileMode = 'blame' | 'history';

/** Requests to show a file's blame or history (from the graph's commit details, Compare…); Source control opens it. */
export const useFileViewRequest = create<{ req: { path: string; mode: FileMode; rev?: string | null; n: number } | null; open(path: string, mode: FileMode, rev?: string | null): void }>((set, get) => ({
  req: null,
  open: (path, mode, rev) => set({ req: { path, mode, rev, n: (get().req?.n ?? 0) + 1 } }),
}));

/** Menu items for a file: blame (at a revision) and history. */
export function fileViewItems(path: string, rev?: string | null, exists = true): { label: string; disabled?: boolean; onSelect(): void }[] {
  return [
    { label: rev ? `Blame at ${rev.slice(0, 7)}` : 'Blame', disabled: !exists, onSelect: () => useFileViewRequest.getState().open(path, 'blame', rev) },
    { label: 'File history', onSelect: () => useFileViewRequest.getState().open(path, 'history') },
  ];
}

const ROW = 18;

/** 0 (oldest) … 4 (newest) for a commit time within the file's range. */
export function ageBucket(time: number, min: number, max: number): number {
  if (max <= min) return 4;
  return Math.max(0, Math.min(4, Math.floor(((time - min) / (max - min)) * 5)));
}

/**
 * Blame and history of one file. Blame: who last changed each line (moved/copied lines followed,
 * whitespace-only changes and `.git-blame-ignore-revs` commits skipped), coloured by age; click a
 * commit to open it in the graph. History: every version (renames followed) with its diff
 * against the version before.
 */
export function FileView({ root, repo, path, mode, rev, onMode, onClose, onOpenCommit }: { root: string; repo: string | null; path: string; mode: FileMode; rev?: string | null; onMode(m: FileMode): void; onClose(): void; onOpenCommit(hash: string): void }) {
  return (
    <div className="col h-full min-h-0" style={{ gap: 0 }}>
      <div className="toolbar">
        {mode === 'blame' ? <ScanText size={14} className="subtle" /> : <FileClock size={14} className="subtle" />}
        <span className="mono xs truncate grow" title={path}>
          {path}
          {rev ? <span className="subtle"> @ {rev.slice(0, 7)}</span> : null}
        </span>
        <Segmented
          value={mode}
          onChange={onMode}
          options={[
            { value: 'blame', label: 'Blame', title: 'Who last changed each line' },
            { value: 'history', label: 'History', title: 'Every version of the file' },
          ]}
        />
        <button className="icon-btn sm" title="Close" onClick={onClose}>
          <X />
        </button>
      </div>
      <div className="grow min-h-0">{mode === 'blame' ? <BlamePane root={root} repo={repo} path={path} rev={rev ?? null} onOpenCommit={onOpenCommit} /> : <HistoryPane root={root} repo={repo} path={path} onOpenCommit={onOpenCommit} />}</div>
    </div>
  );
}

function BlamePane({ root, repo, path, rev, onOpenCommit }: { root: string; repo: string | null; path: string; rev: string | null; onOpenCommit(hash: string): void }) {
  const [blame, setBlame] = useState<Blame | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [scroll, setScroll] = useState({ top: 0, height: 600 });
  const [hover, setHover] = useState<number | null>(null);
  const body = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let alive = true;
    setBlame(null);
    setError(null);
    gitApi
      .blame(root, repo, path, rev)
      .then((b) => alive && setBlame(b))
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [root, repo, path, rev]);

  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setScroll((s) => ({ ...s, height: el.clientHeight })));
    ro.observe(el);
    return () => ro.disconnect();
  }, [blame]);

  const range = useMemo(() => {
    const times = (blame?.commits ?? []).filter((c) => !c.uncommitted).map((c) => c.time);
    return { min: Math.min(...times, Infinity), max: Math.max(...times, 0) };
  }, [blame]);

  if (error) return <Empty icon={<FileWarning />} title="Couldn't blame the file">{error}</Empty>;
  if (!blame) return <Empty icon={<Spinner />} title="Reading the blame…" />;
  if (!blame.lines.length) return <Empty title="Empty file" />;

  const first = Math.max(0, Math.floor(scroll.top / ROW) - 20);
  const last = Math.min(blame.lines.length, Math.ceil((scroll.top + scroll.height) / ROW) + 20);
  const tip = (c: BlameCommit) => (c.uncommitted ? 'Not committed yet' : `${c.short} · ${c.author} <${c.email}>\n${new Date(c.time * 1000).toLocaleString()} (${relTime(c.time * 1000)})\n\n${c.summary}${c.previous ? `\n\nbefore: ${c.previous[0].slice(0, 7)} ${c.previous[1]}` : ''}`);
  const rows: React.ReactNode[] = [];
  for (let i = first; i < last; i++) {
    const l = blame.lines[i];
    const c = blame.commits[l.commit];
    const start = i === 0 || blame.lines[i - 1].commit !== l.commit;
    rows.push(
      <div key={i} className={clsx('git-blame-row', start && 'start', hover === l.commit && 'hl')} style={{ top: i * ROW, height: ROW }}>
        <div
          className={clsx('git-blame-gutter', c.uncommitted ? 'uncommitted' : `age-${ageBucket(c.time, range.min, range.max)}`)}
          title={tip(c)}
          onMouseEnter={() => setHover(l.commit)}
          onMouseLeave={() => setHover(null)}
          onClick={() => !c.uncommitted && onOpenCommit(c.hash)}
        >
          {start && (
            <>
              <span className="mono">{c.uncommitted ? '·······' : c.short}</span>
              <span className="truncate">{c.uncommitted ? 'you, uncommitted' : c.author}</span>
              {!c.uncommitted && <span className="subtle">{relTime(c.time * 1000)}</span>}
            </>
          )}
        </div>
        <span className="git-ln">{i + 1}</span>
        <span className="git-code">{l.text || ' '}</span>
      </div>,
    );
  }
  return (
    <div className="col h-full min-h-0" style={{ gap: 0 }}>
      <div className="xs subtle git-hint row-sm">
        <History size={12} /> {blame.commits.filter((c) => !c.uncommitted).length} commits · moved and copied lines followed · whitespace ignored{blame.ignoredRevs ? ' · .git-blame-ignore-revs applied' : ''} · click a commit to open it
      </div>
      <div className="grow min-h-0 git-blame-scroll" ref={body} onScroll={(e) => setScroll({ top: e.currentTarget.scrollTop, height: e.currentTarget.clientHeight })}>
        <div className="git-blame-rows" style={{ height: blame.lines.length * ROW }}>
          {rows}
        </div>
      </div>
    </div>
  );
}

function HistoryPane({ root, repo, path, onOpenCommit }: { root: string; repo: string | null; path: string; onOpenCommit(hash: string): void }) {
  const [list, setList] = useState<FileVersion[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [w, setW] = useViewState('git.historyW', 340);
  const opts = useCompare((s) => s.opts);

  useEffect(() => {
    let alive = true;
    setList(null);
    gitApi
      .fileHistory(root, repo, path, 500)
      .then((l) => {
        if (!alive) return;
        setList(l);
        setSel(l[0]?.hash ?? null);
      })
      .catch((e) => alive && setError(errorMessage(e)));
    return () => {
      alive = false;
    };
  }, [root, repo, path]);

  if (error) return <Empty icon={<FileWarning />} title="Couldn't read the history">{error}</Empty>;
  if (!list) return <Empty icon={<Spinner />} title="Reading the history…" />;
  if (!list.length) return <Empty title="No committed versions yet" />;
  const v = list.find((x) => x.hash === sel) ?? list[0];
  const old: DiffSide = v.parent && v.status !== 'A' ? { type: 'rev', rev: v.parent, path: v.oldPath } : { type: 'empty' };
  const next: DiffSide = v.status === 'D' ? { type: 'empty' } : { type: 'rev', rev: v.hash, path: v.path };
  return (
    <div className="git-history">
      <div className="git-history-list list" style={{ width: w }}>
        {list.map((x) => (
          <div key={x.hash} className={clsx('list-item git-commit-row', x.hash === v.hash && 'on')} onClick={() => setSel(x.hash)} onDoubleClick={() => onOpenCommit(x.hash)} title={`${x.hash}\nDouble-click to open it in the graph`}>
            <span className="small truncate w-full">{x.subject}</span>
            <span className="xs subtle row-sm">
              <StatusLetter status={x.status} />
              <span className="mono">{x.short}</span> · <span className="truncate">{x.author}</span> · <RelTime ts={x.date} />
            </span>
            {x.oldPath !== x.path && <span className="xs subtle mono truncate w-full">{x.oldPath} → {x.path}</span>}
          </div>
        ))}
      </div>
      <Resizer width={w} onChange={setW} min={220} max={600} defaultValue={340} />
      <div className="git-history-diff">
        <div className="toolbar git-diff-toolbar">
          <span className="mono xs shrink-0">{v.short}</span>
          <span className="small truncate grow">{v.subject}</span>
          <button className="btn btn-sm btn-ghost" onClick={() => onOpenCommit(v.hash)}>
            Open commit
          </button>
        </div>
        <div className="grow min-h-0 git-review-diff">
          <DiffPane root={root} repo={repo} old={old} next={next} path={v.path} opts={opts} />
        </div>
      </div>
    </div>
  );
}
